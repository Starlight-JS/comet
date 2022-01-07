//! # Immix: A mark-region garbage collector
//!
//! The Immix collector is a collector based on mark-and-sweep with a
//! modified thread-local allocation algorithm and a heap layout more optimised for
//! modern CPU caches. The heap is organised into large blocks containing lines
//! which then contain actual objects. The line size is chosen to more or less align
//! with the cache line sizes of modern CPU architectures, such as x86 64. Objects
//! are not collected unless all objects in a line are unreachable, and surprisingly this
//! coarser granularity leads to performance improvements.
//! You can find more information about Immix in this [paper](https://users.cecs.anu.edu.au/~steveb/pubs/papers/immix-pldi-2008.pdf)

use crate::{
    api::{vtable_of, Collectable, Gc, HeapObjectHeader, Trace, Visitor, Weak, GC_BLACK, GC_WHITE},
    gc_base::{AllocationSpace, GcBase, MarkingConstraint, MarkingConstraintRuns, NoReadBarrier},
    large_space::{LargeObjectSpace, PreciseAllocation},
    mutator::{oom_abort, JoinData, Mutator, MutatorRef, ThreadState},
    safepoint::{GlobalSafepoint, SafepointScope},
    small_type_id,
    utils::{align_usize, formatted_size},
};
use crate::{
    bitmap::{round_up, ChunkMap},
    gc_base::TLAB,
    utils::align_down,
};
use atomic::Ordering;
use rosalloc::defs::PAGE_SIZE;
use std::{cell::UnsafeCell, marker::PhantomData, mem::size_of, ptr::NonNull, sync::Arc};
use std::{
    ptr::null_mut,
    sync::atomic::{AtomicPtr, AtomicUsize},
};
pub fn line_align(ptr: *const u8) -> *mut u8 {
    align_down(ptr as _, IMMIX_LINE_SIZE) as _
}

pub fn is_line_aligned(ptr: *const u8) -> bool {
    line_align(ptr) == ptr as *mut u8
}

pub mod block;
pub mod chunk;
pub mod space;
use block::*;
use chunk::*;
use space::*;

/// Thread local allocator for Immix. This allocator stores two different bump pointers:
/// 1) Large cursor for allocating objects that span multiple lines
/// 2) Regular cursor for objects whose size is smaller than [IMMIX_LINE_SIZE](IMMIX_LINE_SIZE).
pub struct ImmixAllocator {
    cursor: *mut u8,
    limit: *mut u8,
    space: &'static ImmixSpace,
    large_cursor: *mut u8,
    large_limit: *mut u8,
    request_for_large: bool,
    emergency_collection: bool,
    line: Option<*mut u8>,
}

impl ImmixAllocator {
    /// Try to acquire recyclable block. Returns false if there is no recyclable blocks or GC threshold is reached.
    pub fn acquire_recyclable_block(&mut self) -> bool {
        if self.is_out_of_memory_on_allocation(IMMIX_BLOCK_SIZE, self.emergency_collection) {
            return false;
        }

        match self.space.get_reusable_block() {
            block if !block.is_null() => {
                self.line = Some(unsafe { (*block).start().add(IMMIX_LINE_SIZE) });
                unsafe {
                    self.space.num_bytes_allocated.fetch_add(
                        match (*block).state() {
                            BlockState::Reusable { unavailable_lines } => {
                                (IMMIX_LINES_PER_BLOCK - unavailable_lines as usize - 1)
                                    * IMMIX_LINE_SIZE
                            }
                            _ => unreachable!(),
                        },
                        Ordering::Relaxed,
                    );
                }
                true
            }
            _ => false,
        }
    }

    /// Acquire recyclable lines from current block. Returns false if there is no more holes in block.
    pub fn acquire_recyclable_lines(&mut self) -> bool {
        while self.line.is_some() || self.acquire_recyclable_block() {
            let line = self.line.unwrap();

            let (start, end) = self.space.acquire_recyclable_lines(line);
            if !start.is_null() && !end.is_null() {
                self.space
                    .num_bytes_allocated
                    .fetch_add(end as usize - start as usize, Ordering::Relaxed);
                self.cursor = start;
                self.limit = end;

                let block = ImmixBlock::align(start).cast::<ImmixBlock>();

                self.line = if unsafe { end == (*block).end() } {
                    // Hole searching reached the end of a reusable block. Set the hole-searching cursor to None.
                    None
                } else {
                    // Update the hole-searching cursor to None.
                    Some(end)
                };
                return true;
            } else {
                // No more recyclable lines. Set the hole-searching cursor to None.
                self.line = None;
            }
        }

        false
    }
    pub fn acquire_clean_block(&mut self) -> bool {
        if self.is_out_of_memory_on_allocation(IMMIX_BLOCK_SIZE, self.emergency_collection) {
            return false;
        }
        match self.space.get_clean_block() {
            block if !block.is_null() => unsafe {
                self.space
                    .num_bytes_allocated
                    .fetch_add(IMMIX_BLOCK_SIZE, Ordering::Relaxed);
                if self.request_for_large {
                    self.large_cursor = (*block).start_address();
                    self.large_limit = (*block).end();
                } else {
                    self.cursor = (*block).start_address();
                    self.limit = (*block).end();
                }

                true
            },
            _ => false,
        }
    }
}

pub trait GetImmixSpace {
    fn immix_space(&self) -> &'static ImmixSpace;
}

impl<H: GcBase<TLAB = Self>> TLAB<H> for ImmixAllocator
where
    H: GetImmixSpace,
{
    fn can_thread_local_allocate(&self, size: usize) -> bool {
        size <= (IMMIX_BLOCK_SIZE >> 1)
    }

    fn refill(&mut self, _mutator: &crate::mutator::MutatorRef<H>, _alloc_size: usize) -> bool {
        false
    }
    fn allocate<T: crate::api::Collectable + 'static>(
        &mut self,
        _value: T,
    ) -> Result<crate::api::Gc<T, H>, T> {
        unreachable!()
    }
    fn reset(&mut self) {
        self.large_cursor = null_mut();
        self.large_limit = null_mut();
        self.cursor = null_mut();
        self.limit = null_mut();
        self.line = None;
    }
    fn create(heap: std::sync::Arc<std::cell::UnsafeCell<H>>) -> Self {
        Self {
            space: unsafe { (*heap.get()).immix_space() },
            line: None,
            limit: null_mut(),
            large_cursor: null_mut(),
            large_limit: null_mut(),
            cursor: null_mut(),
            request_for_large: false,
            emergency_collection: false,
        }
    }
}

impl ImmixAllocator {
    #[inline]
    fn is_out_of_memory_on_allocation(&self, alloc_size: usize, grow: bool) -> bool {
        let mut old_target = self.space.target_footprint.load(Ordering::Relaxed);
        loop {
            let old_allocated = self.space.num_bytes_allocated.load(Ordering::Relaxed);
            let new_footprint = old_allocated + alloc_size;
            if new_footprint <= old_target {
                return false;
            } else if new_footprint > self.space.growth_limit {
                return true;
            }

            if grow {
                if let Err(t) = self.space.target_footprint.compare_exchange_weak(
                    old_target,
                    new_footprint,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    old_target = t;
                    //return false;
                } else {
                    return false;
                }
            } else {
                return true;
            }
        }
    }
    pub unsafe fn alloc_slow_once(&mut self, size: usize) -> *mut u8 {
        if self.acquire_clean_block() {
            self.alloc(size)
        } else {
            null_mut()
        }
    }
    #[inline(always)]
    pub unsafe fn alloc_slow_inline(&mut self, size: usize) -> *mut u8 {
        let result = self.alloc_slow_once(size);
        if !result.is_null() {
            return result;
        }

        if self.emergency_collection {
            oom_abort();
        }
        result
    }
    pub unsafe fn alloc(&mut self, size: usize) -> *mut u8 {
        let result = self.cursor;
        let new_cursor = result.add(size);

        if new_cursor > self.limit {
            if size > IMMIX_LINE_SIZE {
                self.overflow_alloc(size)
            } else {
                self.alloc_slow_hot(size)
            }
        } else {
            self.cursor = new_cursor;
            result
        }
    }

    unsafe fn overflow_alloc(&mut self, size: usize) -> *mut u8 {
        let start = self.large_cursor;
        let end = start.add(size);
        if end > self.large_limit {
            self.request_for_large = true;
            let rtn = self.alloc_slow_inline(size);
            self.request_for_large = false;
            rtn
        } else {
            self.large_cursor = end;
            start
        }
    }

    #[cold]
    unsafe fn alloc_slow_hot(&mut self, size: usize) -> *mut u8 {
        if self.acquire_recyclable_lines() {
            self.alloc(size)
        } else {
            self.alloc_slow_inline(size)
        }
    }
}
use parking_lot::{lock_api::RawMutex, RawMutex as Lock};

/// Immix GC implementation. Read top level module documentation for more information
pub struct Immix {
    space: &'static ImmixSpace,
    pub(crate) global_heap_lock: Lock,
    pub(crate) large_space_lock: Lock,
    pub(crate) large_space: LargeObjectSpace,
    pub(crate) mutators: Vec<*mut Mutator<Self>>,
    pub(crate) safepoint: GlobalSafepoint,
    pub(crate) mark_stack: Vec<*mut HeapObjectHeader>,
    pub(crate) verbose: bool,
    pub(crate) alloc_color: u8,
    pub(crate) mark_color: u8,
    total_gcs: usize,
    weak_refs: Vec<Weak<dyn Collectable, Self>>,
    constraints: Vec<Box<dyn MarkingConstraint>>,
}

impl GetImmixSpace for Immix {
    fn immix_space(&self) -> &'static ImmixSpace {
        self.space
    }
}

pub fn instantiate_immix(
    size: usize,
    initial_size: usize,
    min_heap_size: usize,
    max_heap_size: usize,
    verbose: bool,
) -> MutatorRef<Immix> {
    let space = Box::leak(Box::new(ImmixSpace::new(
        size,
        initial_size,
        min_heap_size,
        max_heap_size,
        verbose,
    )));

    let immix = Arc::new(UnsafeCell::new(Immix {
        space,
        large_space: LargeObjectSpace::new(),
        large_space_lock: Lock::INIT,
        verbose,
        global_heap_lock: Lock::INIT,
        mutators: vec![],
        safepoint: GlobalSafepoint::new(),
        alloc_color: GC_WHITE,
        mark_color: GC_BLACK,
        mark_stack: Vec::new(),
        total_gcs: 0,
        weak_refs: vec![],
        constraints: vec![],
    }));
    let href = unsafe { &mut *immix.get() };
    let join_data = JoinData::new();
    let mut mutator = MutatorRef::new(Mutator::new(
        immix.clone(),
        &href.safepoint,
        join_data.internal.clone(),
    ));
    href.mutators.push(&mut *mutator);
    href.safepoint
        .n_mutators
        .fetch_add(1, atomic::Ordering::Relaxed);
    mutator.state_set(ThreadState::Safe, ThreadState::Unsafe);
    mutator
}

impl Immix {
    unsafe fn after_mark_constraints(&mut self) {
        let this = self as *mut Self;
        (*this).constraints.retain_mut(|constraint| {
            if constraint.is_over() {
                false
            } else {
                if constraint.runs_at() == MarkingConstraintRuns::AfterMark {
                    constraint.run(self);
                }
                true
            }
        });
    }
    unsafe fn before_mark_constraints(&mut self) {
        let this = self as *mut Self;
        (*this).constraints.retain_mut(|constraint| {
            if constraint.is_over() {
                false
            } else {
                if constraint.runs_at() == MarkingConstraintRuns::BeforeMark {
                    constraint.run(self);
                }
                true
            }
        });
    }
    #[cold]
    unsafe fn collect_and_alloc<T: Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,

        mut value: T,
    ) -> Gc<T, Self> {
        self.collect_alloc_failure(mutator, &mut [&mut value]);

        mutator.tlab.emergency_collection = true;
        let value = self.alloc_inline(mutator, value, AllocationSpace::New);
        mutator.tlab.emergency_collection = false;
        value
    }
}

impl GcBase for Immix {
    type TLAB = ImmixAllocator;
    const SUPPORTS_TLAB: bool = false;
    type ReadBarrier = NoReadBarrier;
    const LARGE_ALLOCATION_SIZE: usize = IMMIX_BLOCK_SIZE / 2;
    fn add_constraint<T: MarkingConstraint + 'static>(&mut self, constraint: T) {
        self.global_lock();
        self.constraints.push(Box::new(constraint));
        self.global_unlock();
    }
    fn allocate_weak<T: Collectable + ?Sized>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        value: Gc<T, Self>,
    ) -> Weak<T, Self> {
        let weak_ref = unsafe { Weak::<T, Self>::create(mutator, value) };
        self.global_heap_lock.lock();
        self.weak_refs.push(weak_ref.to_dyn());
        unsafe {
            self.global_heap_lock.unlock();
        }
        weak_ref
    }
    fn alloc_inline<T: Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        value: T,
        _space: AllocationSpace,
    ) -> Gc<T, Self> {
        let alloc = &mut mutator.tlab;
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        unsafe {
            let memory = alloc.alloc(size);

            if memory.is_null() {
                return self.collect_and_alloc(mutator, value);
            }
            let object = memory.cast::<HeapObjectHeader>();
            (*object).set_vtable(vtable_of::<T>());
            (*object).type_id = small_type_id::<T>();
            (*object).set_size(size);
            ((*object).data() as *mut T).write(value);
            let gced = Gc {
                base: NonNull::new_unchecked(object),
                marker: Default::default(),
            };
            self.post_alloc(gced);
            gced
        }
    }
    fn collect(&mut self, mutator: &mut MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        match SafepointScope::new(mutator.clone()) {
            Some(safepoint) => unsafe {
                let time = if self.verbose {
                    Some(std::time::Instant::now())
                } else {
                    None
                };

                self.global_heap_lock.lock();
                self.large_space_lock.lock();

                self.space.prepare(true);
                self.before_mark_constraints();
                for object in keep {
                    object.trace(self);
                }
                for i in 0..self.mutators.len() {
                    let mutator = self.mutators[i];
                    (*mutator).reset_tlab();
                    (*mutator).shadow_stack().walk(|entry| {
                        entry.trace(self);
                    });
                }
                while let Some(object) = self.mark_stack.pop() {
                    (*object).get_dyn().trace(self);
                }
                self.after_mark_constraints();
                let prev =
                    self.space.num_bytes_allocated.load(Ordering::Relaxed) + self.large_space.bytes;
                self.space.num_bytes_allocated.store(0, Ordering::Relaxed);
                let mark_color = self.mark_color;
                self.weak_refs.retain_mut(|object| {
                    let header = object.base();
                    if (*header).get_color() == mark_color {
                        object.after_mark(|header| {
                            if (*header).get_color() == mark_color {
                                header
                            } else {
                                null_mut()
                            }
                        });
                        true
                    } else {
                        false
                    }
                });
                self.large_space.sweep();
                self.large_space.prepare_for_allocation(false);
                self.space.release();

                let bytes_allocated =
                    self.space.num_bytes_allocated.load(Ordering::Relaxed) + self.large_space.bytes;
                let target_size = self
                    .space
                    .min_heap_size
                    .max((bytes_allocated as f64 * 1.75) as usize)
                    .min(self.space.max_heap_size);

                self.space
                    .target_footprint
                    .store(target_size, Ordering::Relaxed);
                if let Some(time) = time {
                    let elapsed = time.elapsed();
                    eprintln!(
                        "[gc] GC({}) Pause Immix collection {}->{}({}) {:.4}ms",
                        self.total_gcs,
                        formatted_size(prev),
                        formatted_size(bytes_allocated),
                        formatted_size(target_size),
                        elapsed.as_micros() as f64 / 1000.0
                    );
                }
                self.total_gcs += 1;
                std::mem::swap(&mut self.alloc_color, &mut self.mark_color);
                drop(safepoint);

                self.global_heap_lock.unlock();
                self.large_space_lock.unlock();
            },
            None => return,
        }
    }

    fn alloc_tlab_area(&mut self, _mutator: &MutatorRef<Self>, _size: usize) -> *mut u8 {
        null_mut()
    }
    fn safepoint(&self) -> &GlobalSafepoint {
        &self.safepoint
    }

    fn attach_current_thread(&mut self, mutator: *mut Mutator<Self>) {
        self.global_heap_lock.lock();
        self.safepoint.n_mutators.fetch_add(1, Ordering::Relaxed);
        self.mutators.push(mutator);
        unsafe { self.global_heap_lock.unlock() };
    }

    fn detach_current_thread(&mut self, mutator: *mut Mutator<Self>) {
        self.global_heap_lock.lock();

        let mut detached = false;
        self.mutators.retain(|x| {
            let x = *x;
            let y = mutator;
            if x == y {
                detached = true;
                false
            } else {
                true
            }
        });
        self.safepoint.n_mutators.fetch_sub(1, Ordering::Relaxed);
        assert!(detached, "mutator must be detached");
        unsafe {
            self.global_heap_lock.unlock();
        }
    }

    fn global_lock(&self) {
        self.global_heap_lock.lock();
    }
    fn global_unlock(&self) {
        unsafe {
            debug_assert!(self.global_heap_lock.is_locked());
            self.global_heap_lock.unlock();
        }
    }

    fn mutators(&self) -> &[*mut Mutator<Self>] {
        assert!(self.global_heap_lock.is_locked());
        &self.mutators
    }

    fn allocate_large<T: Collectable + Sized + 'static>(
        &mut self,
        _mutator: &mut MutatorRef<Self>,
        value: T,
    ) -> crate::api::Gc<T, Self> {
        unsafe {
            let size = value.allocation_size() + size_of::<HeapObjectHeader>();
            self.large_space_lock.lock();
            let object = self.large_space.allocate(size);
            (*object).set_vtable(vtable_of::<T>());
            (*object).type_id = small_type_id::<T>();
            let gc = Gc {
                base: NonNull::new_unchecked(object),
                marker: PhantomData,
            };
            ((*object).data() as *mut T).write(value);
            self.large_space_lock.unlock();
            self.post_alloc(gc);
            gc
        }
    }
    #[inline(always)]
    fn post_alloc<T: Collectable + Sized + 'static>(&mut self, value: Gc<T, Self>) {
        unsafe {
            let base = value.base.as_ptr();
            (*base).force_set_color(self.alloc_color);
        }
    }
}

impl Visitor for Immix {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        let object = root.as_ptr();
        unsafe {
            // todo: opportunistic evacuation

            if !(*object).set_color(self.alloc_color, self.mark_color) {
                if self.space.has_address(object.cast()) {
                    self.space.mark_lines(object);
                } else {
                    (*PreciseAllocation::from_cell(object)).test_and_set_marked();
                }
                self.mark_stack.push(object);
            }
        }
    }
}

/*
pub type Histogram = [usize; Defrag::NUM_BINS];

pub struct Defrag {
    in_defrag_collection: AtomicBool,
    defrag_space_exhausted: AtomicBool,
    pub mark_histograms: Mutex<Vec<Histogram>>,
    /// A block with number of holes greater than this threshold will be defragmented.
    pub defrag_spill_threshold: AtomicUsize,
    /// The number of remaining clean pages in defrag space.
    available_clean_pages_for_defrag: AtomicUsize,
}
impl Defrag {
    const NUM_BINS: usize = (IMMIX_LINES_PER_BLOCK >> 1) + 1;
    const DEFRAG_LINE_REUSE_RATIO: f32 = 0.99;
    const MIN_SPILL_THRESHOLD: usize = 2;
    const DEFRAG_STRESS: bool = false;
    const DEFRAG_HEADROOM_PERCENT: usize = 2;

    /// Allocate a new local histogram.
    pub const fn new_histogram(&self) -> Histogram {
        [0; Self::NUM_BINS]
    }

    /// Report back a completed mark histogram
    #[inline(always)]
    pub fn add_completed_mark_histogram(&self, histogram: Histogram) {
        self.mark_histograms.lock().push(histogram)
    }

    /// Check if the current GC is a defrag GC.
    #[inline(always)]
    pub fn in_defrag(&self) -> bool {
        self.in_defrag_collection.load(Ordering::Acquire)
    }

    /// Determine whether the current GC should do defragmentation.
    pub fn decide_whether_to_defrag(
        &self,
        emergency_collection: bool,
        collect_whole_heap: bool,
        collection_attempts: usize,
        user_triggered: bool,
        exhausted_reusable_space: bool,
        full_heap_system_gc: bool,
    ) {
        let in_defrag = true
            && (emergency_collection
                || (collection_attempts > 1)
                || !exhausted_reusable_space
                || Self::DEFRAG_STRESS
                || (collect_whole_heap && user_triggered && full_heap_system_gc));
        // println!("Defrag: {}", in_defrag);
        self.in_defrag_collection
            .store(in_defrag, Ordering::Release)
    }

    /// Get the number of defrag headroom pages.
    pub fn defrag_headroom_pages(&self, space: &ImmixSpace) -> usize {
        space.get_reserved_pages() * Self::DEFRAG_HEADROOM_PERCENT / 100
    }

    /// Check if the defrag space is exhausted.
    #[inline(always)]
    pub fn space_exhausted(&self) -> bool {
        self.defrag_space_exhausted.load(Ordering::Acquire)
    }

    /// Update available_clean_pages_for_defrag counter when a clean block is allocated.
    pub fn notify_new_clean_block(&self, copy: bool) {
        if copy {
            let available_clean_pages_for_defrag =
                self.available_clean_pages_for_defrag.fetch_update(
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                    |available_clean_pages_for_defrag| {
                        if available_clean_pages_for_defrag <= IMMIX_BLOCK_SIZE / PAGE_SIZE {
                            Some(0)
                        } else {
                            Some(available_clean_pages_for_defrag - (IMMIX_BLOCK_SIZE / PAGE_SIZE))
                        }
                    },
                );
            if available_clean_pages_for_defrag.unwrap() <= IMMIX_BLOCK_SIZE / PAGE_SIZE {
                self.defrag_space_exhausted.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Prepare work. Should be called in ImmixSpace::prepare.
    #[allow(clippy::assertions_on_constants)]
    pub fn prepare(&self, space: &ImmixSpace<VM>) {
        self.defrag_space_exhausted.store(false, Ordering::Release);

        // Calculate available free space for defragmentation.

        let mut available_clean_pages_for_defrag = VM::VMActivePlan::global().get_total_pages()
            as isize
            - VM::VMActivePlan::global().get_pages_reserved() as isize
            + self.defrag_headroom_pages(space) as isize;
        if available_clean_pages_for_defrag < 0 {
            available_clean_pages_for_defrag = 0
        };

        self.available_clean_pages_for_defrag
            .store(available_clean_pages_for_defrag as usize, Ordering::Release);

        if self.in_defrag() {
            self.establish_defrag_spill_threshold(space)
        }

        self.available_clean_pages_for_defrag.store(
            available_clean_pages_for_defrag as usize
                + VM::VMActivePlan::global().get_collection_reserve(),
            Ordering::Release,
        );
    }

    /// Get the numebr of all the recyclable lines in all the reusable blocks.
    fn get_available_lines(
        &self,
        space: &ImmixSpace<VM>,
        spill_avail_histograms: &mut Histogram,
    ) -> usize {
        let mut total_available_lines = 0;
        for block in space.reusable_blocks.get_blocks().iter() {
            let bucket = block.get_holes();
            let unavailable_lines = match block.get_state() {
                BlockState::Reusable { unavailable_lines } => unavailable_lines as usize,
                s => unreachable!("{:?} {:?}", block, s),
            };
            let available_lines = Block::LINES - unavailable_lines;
            spill_avail_histograms[bucket] += available_lines;
            total_available_lines += available_lines;
        }
        total_available_lines
    }

    /// Calculate the defrag threshold.
    fn establish_defrag_spill_threshold(&self, space: &ImmixSpace) {
        let mut spill_avail_histograms = self.new_histogram();
        let clean_lines = self.get_available_lines(space, &mut spill_avail_histograms);
        let available_lines = clean_lines
            + (self
                .available_clean_pages_for_defrag
                .load(Ordering::Acquire)
                << (12 as usize - 8));

        // Number of lines we will evacuate.
        let mut required_lines = 0isize;
        // Number of to-space free lines we can use for defragmentation.
        let mut limit = (available_lines as f32 / Self::DEFRAG_LINE_REUSE_RATIO) as isize;
        let mut threshold = IMMIX_LINES_PER_BLOCK >> 1;
        let mark_histograms = self.mark_histograms.lock();
        // Blocks are grouped by buckets, indexed by the number of holes in the block.
        // `mark_histograms` remembers the number of live lines for each bucket.
        // Here, reversely iterate all the bucket to find a threshold that all buckets above this
        // threshold can be evacuated, without causing to-space overflow.
        for index in (Self::MIN_SPILL_THRESHOLD..Self::NUM_BINS).rev() {
            threshold = index;
            // Calculate total number of live lines in this bucket.
            let this_bucket_mark = mark_histograms
                .iter()
                .map(|v| v[threshold] as isize)
                .sum::<isize>();
            // Calculate the number of free lines in this bucket.
            let this_bucket_avail = spill_avail_histograms[threshold] as isize;
            // Update counters
            limit -= this_bucket_avail as isize;
            required_lines += this_bucket_mark;
            // Stop scanning. Lines to evacuate exceeds the free to-space lines.
            if limit < required_lines {
                break;
            }
        }
        // println!("threshold: {}", threshold);
        debug_assert!(threshold >= Self::MIN_SPILL_THRESHOLD);
        self.defrag_spill_threshold
            .store(threshold, Ordering::Release);
    }

    /// Release work. Should be called in ImmixSpace::release.
    #[allow(clippy::assertions_on_constants)]
    pub fn release(&self, _space: &ImmixSpace) {
        self.in_defrag_collection.store(false, Ordering::Release);
    }
}*/

impl Drop for Immix {
    fn drop(&mut self) {
        unsafe {
            Box::from_raw(self.space as *const ImmixSpace as *mut ImmixSpace);
        }
    }
}
