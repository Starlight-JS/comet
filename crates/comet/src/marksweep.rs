use crate::api::Weak;
use crate::bitmap::SpaceBitmap;
use crate::gc_base::{AllocationSpace, MarkingConstraint, MarkingConstraintRuns, NoReadBarrier};
use crate::rosalloc_space::{RosAllocSpace, RosAllocTLAB};
use crate::utils::formatted_size;
use crate::{
    api::{vtable_of, Collectable, Gc, HeapObjectHeader, Trace, Visitor},
    gc_base::GcBase,
    large_space::{LargeObjectSpace, PreciseAllocation},
    mutator::{oom_abort, JoinData, Mutator, MutatorRef, ThreadState},
    safepoint::{GlobalSafepoint, SafepointScope},
    small_type_id,
    utils::align_usize,
};
use atomic::Ordering;
use parking_lot::{lock_api::RawMutex, RawMutex as Lock};
use rosalloc::{Rosalloc, NUM_OF_SLOTS};
use std::ptr::null_mut;
use std::sync::atomic::AtomicUsize;
use std::{cell::UnsafeCell, marker::PhantomData, mem::size_of, ptr::NonNull, sync::Arc};

#[repr(C)]
pub struct MarkSweep {
    pub(crate) global_heap_lock: Lock,
    pub(crate) large_space_lock: Lock,
    live_bitmap: *const SpaceBitmap<8>,
    rosalloc: *mut RosAllocSpace,
    large_space: LargeObjectSpace,
    mutators: Vec<*mut Mutator<Self>>,
    safepoint: GlobalSafepoint,
    mark_stack: Vec<*mut HeapObjectHeader>,
    target_footprint: AtomicUsize,
    num_bytes_allocated: AtomicUsize,
    growth_limit: usize,
    growth_multiplier: f64,
    max_free: usize,
    min_free: usize,
    pool: scoped_threadpool::Pool,
    verbose: bool,
    total_gcs: usize,
    weak_refs: Vec<Weak<dyn Collectable, Self>>,
    constraints: Vec<Box<dyn MarkingConstraint>>,
}
fn max_bytes_bulk_allocated_for(size: usize) -> usize {
    if !Rosalloc::is_size_for_thread_local(size) {
        return size;
    }
    let (idx, bracket_size) = Rosalloc::size_to_index_and_bracket_size(size);
    NUM_OF_SLOTS[idx] * bracket_size
}

pub fn instantiate_marksweep(
    initial_size: usize,
    growth_limit: usize,
    min_free: usize,
    max_free: usize,
    growht_multiplier: f64,
    capacity: usize,
    low_memory_mode: bool,
    num_threads: usize,
    verbose: bool,
) -> MutatorRef<MarkSweep> {
    let heap = Arc::new(UnsafeCell::new(MarkSweep::new(
        initial_size,
        growth_limit,
        min_free,
        max_free,
        growht_multiplier,
        capacity,
        low_memory_mode,
        num_threads,
        verbose,
    )));
    let href = unsafe { &mut *heap.get() };
    let join_data = JoinData::new();
    let mut mutator = MutatorRef::new(Mutator::new(
        heap.clone(),
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

pub const MS_DEFAULT_INITIAL_SIZE: usize = 2 * 1024 * 1024;
pub const MS_DEFAULT_MAXIMUM_SIZE: usize = 256 * 1024 * 1024;
pub const MS_DEFAULT_MAX_FREE: usize = 2 * 1024 * 1024;
pub const MS_DEFAULT_MIN_FREE: usize = MS_DEFAULT_MAX_FREE / 4;

impl MarkSweep {
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
    pub fn new(
        initial_size: usize,
        growth_limit: usize,
        min_free: usize,
        max_free: usize,
        growth_multiplier: f64,
        capacity: usize,
        low_memory_mode: bool,
        num_threads: usize,
        verbose: bool,
    ) -> Self {
        let growth_limit = capacity.min(growth_limit);
        let rosalloc = RosAllocSpace::create(
            "mark-sweep",
            initial_size,
            growth_limit,
            capacity,
            low_memory_mode,
            false,
        );
        let this = Self {
            total_gcs: 0,
            constraints: vec![],
            global_heap_lock: Lock::INIT,
            large_space_lock: Lock::INIT,
            live_bitmap: unsafe { (*rosalloc).get_live_bitmap() },
            rosalloc,
            large_space: LargeObjectSpace::new(),
            mutators: vec![],
            safepoint: GlobalSafepoint::new(),
            mark_stack: vec![],
            target_footprint: AtomicUsize::new(initial_size),
            max_free,
            min_free,
            growth_limit,
            num_bytes_allocated: AtomicUsize::new(0),
            growth_multiplier,
            pool: scoped_threadpool::Pool::new(num_threads as _),
            verbose,
            weak_refs: vec![],
        };
        unsafe {
            (*(*this.rosalloc).rosalloc()).set_footprint_limit((*this.rosalloc).capacity());
        }

        this
    }

    #[inline]
    fn is_out_of_memory_on_allocation(&self, alloc_size: usize, grow: bool) -> bool {
        let mut old_target = self.target_footprint.load(Ordering::Relaxed);
        loop {
            let old_allocated = self.num_bytes_allocated.load(Ordering::Relaxed);
            let new_footprint = old_allocated + alloc_size;
            if new_footprint <= old_target {
                return false;
            } else if new_footprint > self.growth_limit {
                return true;
            }

            if grow {
                if let Err(t) = self.target_footprint.compare_exchange_weak(
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

    #[cold]
    pub fn alloc_slow<T: Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        mut value: T,
    ) -> Gc<T, Self> {
        self.collect(mutator, &mut [&mut value]);
        self.alloc_once::<T, true, false>(mutator, value)
    }
    #[inline(never)]
    pub fn alloc_once<T: Collectable + Sized + 'static, const GROW: bool, const GC: bool>(
        &mut self,
        mut mutator: &mut MutatorRef<Self>,
        value: T,
    ) -> Gc<T, Self> {
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        let max_bytes_tl_bulk_allocated = max_bytes_bulk_allocated_for(size);
        if self.is_out_of_memory_on_allocation(max_bytes_tl_bulk_allocated, GROW) {
            // potentially run GC if we reached GC threshold

            return self.alloc_slow(mutator, value);
        }
        let mut bytes_allocated = 0;
        let mut usable_size = 0;
        let mut bytes_tl_bulk_allocated = 0;
        unsafe {
            let mem = (*self.rosalloc).alloc_common::<Self, true>(
                &mut mutator,
                size,
                &mut bytes_allocated,
                &mut usable_size,
                &mut bytes_tl_bulk_allocated,
            );
            if mem.is_null() && GC {
                // trigger GC if no memory is available
                return self.alloc_slow(mutator, value);
            } else if mem.is_null() && !GC {
                // if GC hapenned and memory is still unavailbe just OOM
                oom_abort();
            }
            if bytes_tl_bulk_allocated > 0 {
                // update num_bytes_allocated so we can start GC when necessary
                self.num_bytes_allocated
                    .fetch_add(bytes_tl_bulk_allocated, Ordering::Relaxed);
            }

            let header = mem.cast::<HeapObjectHeader>();
            (*header).set_vtable(vtable_of::<T>());
            (*header).set_size(size);
            ((*header).data() as *mut T).write(value);
            (*self.live_bitmap).set(header.cast());
            Gc {
                base: NonNull::new_unchecked(header),
                marker: PhantomData,
            }
        }
    }
}

impl GcBase for MarkSweep {
    type TLAB = RosAllocTLAB;
    const SUPPORTS_TLAB: bool = false;
    type ReadBarrier = NoReadBarrier;
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
        let weak_ref = unsafe { Weak::create(mutator, value) };
        self.global_heap_lock.lock();
        self.weak_refs.push(weak_ref.to_dyn());
        unsafe {
            self.global_heap_lock.unlock();
        }
        weak_ref
    }
    fn collect(&mut self, mutator: &mut MutatorRef<MarkSweep>, mut keep: &mut [&mut dyn Trace]) {
        match SafepointScope::new(mutator.clone()) {
            Some(safepoint) => unsafe {
                self.global_heap_lock.lock();
                self.large_space_lock.lock();
                let time = if self.verbose {
                    Some(std::time::Instant::now())
                } else {
                    None
                };

                let prev = self.num_bytes_allocated.load(Ordering::Relaxed);
                self.large_space.prepare_for_marking(false);
                self.before_mark_constraints();
                for i in 0..self.mutators.len() {
                    let mutator = self.mutators[i];
                    //fill_region((*mutator).tlab.cursor, (*mutator).tlab_end);

                    //  (*mutator).reset_tlab();

                    (*mutator).shadow_stack().walk(|object| {
                        object.trace(self);
                    });
                }
                keep.trace(self);

                while let Some(object) = self.mark_stack.pop() {
                    (*object).get_dyn().trace(self);
                }
                self.after_mark_constraints();
                let rosalloc = self.rosalloc;

                let mark = &*(*rosalloc).get_mark_bitmap();
                self.weak_refs.retain_mut(|object| {
                    let header = object.base();
                    if mark.test(header.cast()) {
                        object.after_mark(|header| {
                            if mark.test(header.cast()) {
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

                let mut revoke_freed = 0;
                for i in 0..self.mutators.len() {
                    let mutator = self.mutators[i];
                    revoke_freed += (*(*self.rosalloc).rosalloc())
                        .revoke_thread_local_runs(&mut (*mutator).tlab.runs);
                }
                (*(*self.rosalloc).rosalloc()).revoke_thread_unsafe_current_runs();

                let (freed, _) = (*self.rosalloc).sweep(false, |pointers, _| {
                    (*self.rosalloc).sweep_callback(pointers, false)
                });
                /*let freed =
                crate::sweeper::rosalloc_parallel_sweep(&mut self.pool, self.rosalloc);*/

                let los_freed = self.large_space.sweep();

                let freed = freed + los_freed + revoke_freed;

                self.num_bytes_allocated.fetch_sub(freed, Ordering::Relaxed);
                (*self.rosalloc).swap_bitmaps();
                (*self.rosalloc).mark_bitmap.clear_all();

                (*(*self.rosalloc).rosalloc()).trim();

                let target_size;
                let bytes_allocated = self.num_bytes_allocated.load(Ordering::Relaxed);
                let mut grow_bytes;
                let delta = (bytes_allocated as f64 * (1.0 / 0.75 - 1.0)) as usize;
                grow_bytes = delta.min(self.max_free);
                grow_bytes = grow_bytes.max(self.min_free);
                target_size = bytes_allocated + (grow_bytes as f64 * 2.0) as usize;
                if let Some(time) = time.map(|x| x.elapsed()) {
                    eprintln!(
                        "[gc] GC({}) Pause MarkSweep {}->{}({}) {:.4}ms",
                        self.total_gcs,
                        formatted_size(prev),
                        formatted_size(bytes_allocated),
                        formatted_size(target_size),
                        time.as_micros() as f64 / 1000.0
                    );
                    self.total_gcs += 1;
                }
                self.large_space.prepare_for_allocation(false);
                self.target_footprint.store(target_size, Ordering::Relaxed);
                drop(safepoint);

                self.global_heap_lock.unlock();
                self.large_space_lock.unlock();
            },
            None => {
                return;
            }
        }
    }
    #[inline(always)]
    fn alloc_inline<T: Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        value: T,
        _space: AllocationSpace,
    ) -> Gc<T, Self> {
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        if Rosalloc::is_size_for_thread_local(size) {
            let obj = unsafe { mutator.allocate_from_tlab(value) };
            match obj {
                Ok(value) => {
                    unsafe {
                        (*self.live_bitmap).set(value.base.as_ptr().cast());
                    }

                    value
                }
                Err(value) => self.alloc_once::<T, false, true>(mutator, value),
            }
        } else {
            self.alloc_once::<T, false, true>(mutator, value)
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
        assert!(detached, "mutator must be detached");
        unsafe {
            self.safepoint.n_mutators.fetch_sub(1, Ordering::Relaxed);
            (*(*self.rosalloc).rosalloc()).revoke_thread_local_runs(&mut (*mutator).tlab.runs);
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
            self.num_bytes_allocated.fetch_add(
                (*PreciseAllocation::from_cell(object)).cell_size(),
                Ordering::Relaxed,
            );
            self.large_space_lock.unlock();
            self.post_alloc(gc);
            gc
        }
    }
    fn init_tlab(&mut self, tlab: &mut Self::TLAB) {
        tlab.rosalloc = self.rosalloc;
    }
}

impl Visitor for MarkSweep {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        let object = root.as_ptr();
        unsafe {
            if (*object).is_precise() {
                if !(*PreciseAllocation::from_cell(object)).test_and_set_marked() {
                    self.mark_stack.push(object);
                }
            } else {
                // If object is not in LOS it must be in rosalloc space
                debug_assert!((*self.rosalloc).has_address(object.cast()));
                let bitmap = (*self.rosalloc).get_mark_bitmap();
                debug_assert!((*(*self.rosalloc).get_live_bitmap()).test(object.cast()));
                if !(*bitmap).set_sync(object.cast()) {
                    self.mark_stack.push(object);
                }
            }
        }
    }
}
