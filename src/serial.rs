use crate::api::vtable_of;
use crate::api::Gc;
use crate::gc_base::GcBase;
use crate::large_space::LargeObjectSpace;
use crate::mutator::*;
use crate::safepoint::*;
use crate::small_type_id;
use crate::tlab::SimpleTLAB;
use crate::utils::align_usize;
use crate::{
    api::{HeapObjectHeader, Trace, Visitor},
    bump_pointer_space::BumpPointerSpace,
    large_space::PreciseAllocation,
    rosalloc_space::RosAllocSpace,
    utils::formatted_size,
};
use atomic::Ordering;
use parking_lot::{lock_api::RawMutex, RawMutex as Lock};
use rosalloc::Rosalloc;
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::size_of;
use std::ptr::{null_mut, NonNull};
use std::sync::Arc;
use threadfin::Task;
use threadfin::ThreadPool;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GcReason {
    RequestedByUser,
    AllocationFailure,
    OldSpaceFull,
}

pub struct Serial {
    nursery: BumpPointerSpace,
    pub(crate) global_heap_lock: Lock,
    pub(crate) large_space_lock: Lock,
    mutators: Vec<*mut Mutator<Self>>,
    safepoint: GlobalSafepoint,
    large_space: LargeObjectSpace,
    old_space: *mut RosAllocSpace,

    verbose: bool,

    mark_stack: Vec<*mut HeapObjectHeader>,

    num_old_space_allocated: usize,

    total_gcs: usize,
    remembered_set: Vec<*mut HeapObjectHeader>,
    rem_set_lock: Lock,

    major_collection_threshold: f64,
    next_major_collection_threshold: usize,
    next_major_collection_initial: usize,
    min_heap_size: usize,
    growth_rate_max: f64,
    promoted: usize,
    pool: threadfin::ThreadPool,
    sweep_task: Option<Task<(f64, usize)>>,
}

pub struct SerialOptions {
    pub verbose: bool,
    pub nursery_size: usize,
    pub initial_size: usize,
    pub growth_limit: usize,
    pub min_heap_size: usize,
    pub capacity: usize,
    pub low_memory_mode: bool,
    pub growth_rate_max: f64,
}

impl Default for SerialOptions {
    fn default() -> Self {
        Self {
            verbose: false,
            nursery_size: 32 * 1024 * 1024,
            initial_size: 128 * 1024 * 1024,
            growth_limit: 512 * 1024 * 1024,
            min_heap_size: 64 * 1024 * 1024,
            capacity: 512 * 1024 * 1024,
            low_memory_mode: false,
            growth_rate_max: 1.4,
        }
    }
}

pub fn instantiate_serial(options: SerialOptions) -> MutatorRef<Serial> {
    let heap = Arc::new(UnsafeCell::new(Serial::new(
        options.verbose,
        Some(options.nursery_size),
        options.initial_size,
        options.growth_limit,
        options.min_heap_size,
        options.capacity,
        options.low_memory_mode,
        Some(options.growth_rate_max),
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

impl Serial {
    fn new(
        verbose: bool,
        nursery_size: Option<usize>,
        initial_size: usize,
        growth_limit: usize,
        min_heap_size: usize,
        capacity: usize,
        low_memory_mode: bool,
        growth_rate_max: Option<f64>,
    ) -> Self {
        let growth_limit = capacity.min(growth_limit);
        let rosalloc = RosAllocSpace::create(
            "old-space",
            initial_size,
            growth_limit,
            capacity,
            low_memory_mode,
            false,
        );

        let mut this = Self {
            sweep_task: None,
            // atm only one concurrent sweep task is spawned
            pool: ThreadPool::builder().size(1).stack_size(8 * 1024).build(),
            global_heap_lock: Lock::INIT,
            large_space_lock: Lock::INIT,
            large_space: LargeObjectSpace::new(),
            mutators: vec![],
            promoted: 0,
            safepoint: GlobalSafepoint::new(),
            mark_stack: vec![],
            remembered_set: vec![],
            rem_set_lock: Lock::INIT,
            nursery: BumpPointerSpace::new(nursery_size.unwrap_or_else(|| 32 * 1024 * 1024)),
            verbose,
            total_gcs: 0,
            min_heap_size,
            growth_rate_max: growth_rate_max.unwrap_or_else(|| 1.4),
            major_collection_threshold: 1.82,
            next_major_collection_initial: 0,
            next_major_collection_threshold: 0,
            num_old_space_allocated: 0,
            old_space: rosalloc,
        };
        this.min_heap_size = this
            .min_heap_size
            .max((this.nursery.size() as f64 * this.major_collection_threshold) as usize);

        this.next_major_collection_initial = this.min_heap_size;
        this.next_major_collection_threshold = this.min_heap_size;
        this.set_major_threshold_from(0.0);
        this
    }

    unsafe fn trace_drag_out(
        &mut self,
        root: &mut NonNull<HeapObjectHeader>,
        _parent: *mut HeapObjectHeader,
    ) {
        // todo: use `parent` for object pinning support
        let object = root.as_ptr();

        if self.nursery.contains(object.cast()) {
            if (*object).is_forwarded() {
                *root = NonNull::new_unchecked((*object).vtable() as _);
            } else {
                let size = (*object).size();
                let mut tl_bulk_allocated = 0;

                let memory = if size > Rosalloc::LARGE_SIZE_THRESHOLD {
                    (*(*self.old_space).rosalloc()).alloc_large_object(
                        size,
                        &mut 0,
                        &mut 0,
                        &mut tl_bulk_allocated,
                    )
                } else {
                    // this malloc call does not have any mutex locks, during minor GC all threads are suspended
                    // and objects can end up in old_space only from this function, in case we add parallel copying we would need to use
                    // `Rosalloc::alloc::<true>()` which enables thread safety
                    (*(*self.old_space).rosalloc()).alloc_from_run_thread_unsafe(
                        size,
                        &mut 0,
                        &mut 0,
                        &mut tl_bulk_allocated,
                    )
                };
                if memory.is_null() {
                    promotion_oom(size, object.cast());
                }

                self.promoted += size;
                (*(*self.old_space).get_live_bitmap()).set(memory);
                {
                    // copy young object to memory in old space and update forwarding pointer
                    std::ptr::copy_nonoverlapping(object.cast::<u8>(), memory.cast::<u8>(), size);
                    (*object).set_forwarded(memory as _);
                    *root = NonNull::new_unchecked(memory.cast());
                }
                // incremase num_old_space_allocated. If at the end of minor collection it is larger than target footprint we perform major collection
                self.num_old_space_allocated += tl_bulk_allocated;
                self.mark_stack.push(memory.cast());
            }
        } else if (*object).is_precise() {
            // To promote LOS object we just set it as marked. During sweeping LOS at minor collection we do not clear these marks
            // but before performing major collection we do clear them so they can be finally be sweeped
            if !(*PreciseAllocation::from_cell(object)).test_and_set_marked() {
                self.promoted += (*PreciseAllocation::from_cell(object)).cell_size();
                self.mark_stack.push(object);
            }
        } else {
            debug_assert!((*self.old_space).has_address(object.cast()));
        }
    }

    unsafe fn trace(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        let object = root.as_ptr();

        if (*object).is_precise() {
            if !(*PreciseAllocation::from_cell(object)).test_and_set_marked() {
                self.mark_stack.push(object);
            }
        } else if (*self.old_space).has_address(object.cast()) {
            if !(*(*self.old_space).get_mark_bitmap()).set(object.cast()) {
                self.mark_stack.push(object);
            }
        } else if self.nursery.contains(object.cast()) {
            // todo: support for conservative scanning means nursery objects might be found in major collections
            // add mark bitmap for nursery so we can work with them properly.
        }
    }

    unsafe fn minor(&mut self, keep: &mut [&mut dyn Trace], reason: GcReason) -> bool {
        // threads must be suspended already
        let time = if self.verbose {
            Some(std::time::Instant::now())
        } else {
            None
        };
        let (conc_sweep, bytes) = if let Some(sweep_task) = self.sweep_task.take() {
            let (time, freed) = sweep_task.join();
            let prev = self.num_old_space_allocated;
            self.num_old_space_allocated -= freed;
            if self.verbose {
                eprintln!(
                    "[gc] Concurrent sweep end: {}->{} {:.4}ms",
                    formatted_size(prev),
                    formatted_size(self.num_old_space_allocated),
                    time
                );
            }
            let total_bytes = self.num_old_space_allocated + self.large_space.bytes;
            self.set_major_threshold_from(total_bytes as f64 * self.major_collection_threshold);
            (false, prev)
        } else {
            (false, 0)
        };

        self.large_space.prepare_for_marking(true);
        self.large_space.begin_marking(false);
        (*(*self.old_space).rosalloc()).revoke_thread_unsafe_current_runs();
        keep.iter_mut().for_each(|item| {
            item.trace(&mut YoungVisitor {
                serial: self,
                parent_object: null_mut(),
            });
        });

        for i in 0..self.mutators.len() {
            let mutator = self.mutators[i];
            (*mutator).reset_tlab();
            (*mutator).shadow_stack().walk(|var| {
                var.trace(&mut YoungVisitor {
                    serial: self,
                    parent_object: null_mut(),
                });
            });
        }

        while let Some(object) = self.remembered_set.pop() {
            (*object).get_dyn().trace(&mut YoungVisitor {
                serial: self,
                parent_object: object,
            });
            (*object).unmark();
        }

        while let Some(object) = self.mark_stack.pop() {
            (*object).get_dyn().trace(&mut YoungVisitor {
                serial: self,
                parent_object: object,
            });
        }
        self.large_space.prepare_for_allocation(true);
        self.large_space.sweep();

        self.nursery.reset();
        if conc_sweep {
            let total_bytes = bytes + self.large_space.bytes;
            self.set_major_threshold_from(total_bytes as f64 * self.major_collection_threshold);
        }
        if let Some(time) = time {
            let elapsed = time.elapsed();
            eprintln!(
                "[gc] GC({}) Pause Young ({:?}) Promoted {}(old space: {}) {:.4}ms",
                self.total_gcs,
                reason,
                formatted_size(self.promoted),
                formatted_size(self.num_old_space_allocated + self.large_space.bytes),
                elapsed.as_micros() as f64 / 1000.0
            )
        }
        self.total_gcs += 1;
        self.promoted = 0;
        self.large_space.bytes + self.num_old_space_allocated > self.next_major_collection_threshold
    }

    unsafe fn major(&mut self, keep: &mut [&mut dyn Trace], reason: GcReason) {
        let time = if self.verbose {
            Some(std::time::Instant::now())
        } else {
            None
        };

        self.large_space.prepare_for_marking(false);
        self.large_space.begin_marking(true);
        keep.iter_mut().for_each(|item| {
            item.trace(&mut OldVisitor { serial: self });
        });

        for i in 0..self.mutators.len() {
            let mutator = self.mutators[i];
            (*mutator).reset_tlab();
            (*mutator)
                .shadow_stack()
                .walk(|var| var.trace(&mut OldVisitor { serial: self }));
        }

        while let Some(object) = self.remembered_set.pop() {
            (*object).get_dyn().trace(&mut OldVisitor { serial: self });
            (*object).unmark();
        }

        while let Some(object) = self.mark_stack.pop() {
            (*object).get_dyn().trace(&mut OldVisitor { serial: self });
        }
        /*let prev = self.num_old_space_allocated + self.large_space.bytes;
        let (freed, _) = (*self.old_space).sweep(false, |pointers, _| {
            (*self.old_space).sweep_callback(pointers, false)
        });*/
        //self.num_old_space_allocated -= freed;
        self.large_space.prepare_for_allocation(false);
        self.large_space.sweep();
        /* self.set_major_threshold_from(
            (self.num_old_space_allocated + self.large_space.bytes) as f64
                * self.major_collection_threshold,
        );*/

        let rosalloc = self.old_space as usize;
        self.sweep_task = Some(self.pool.execute(move || {
            let start = std::time::Instant::now();
            let rosalloc = rosalloc as *mut RosAllocSpace;
            (*(*rosalloc).rosalloc()).revoke_thread_unsafe_current_runs();
            eprintln!(
                "[gc] Start concurrent sweep {:p}->{:p} {}",
                (*rosalloc).begin(),
                (*rosalloc).end(),
                formatted_size((*rosalloc).end() as usize - (*rosalloc).begin() as usize)
            );
            let freed = (*rosalloc)
                .sweep(false, |pointers, _| {
                    (*rosalloc).sweep_callback(pointers, false)
                })
                .0;
            (*(*rosalloc).rosalloc()).trim();
            let elapsed = start.elapsed().as_micros() as f64 / 1000.0;
            (elapsed, freed)
        }));
        if let Some(time) = time {
            eprintln!(
                "[gc] GC({}) Pause Old ({:?}) {}({}) {:.4}ms",
                self.total_gcs,
                reason,
                formatted_size(self.num_old_space_allocated + self.large_space.bytes),
                formatted_size(self.next_major_collection_threshold),
                time.elapsed().as_micros() as f64 / 1000.0
            )
        }
        self.total_gcs += 1;
    }

    fn set_major_threshold_from(&mut self, mut threshold: f64) {
        let threshold_max =
            (self.next_major_collection_initial as f64 * self.growth_rate_max) as usize;

        if threshold > threshold_max as f64 {
            threshold = threshold_max as _;
        }
        if threshold < self.min_heap_size as f64 {
            threshold = self.min_heap_size as _;
        }
        self.next_major_collection_initial = threshold as _;
        self.next_major_collection_threshold = threshold as _;
        if self.verbose {
            eprintln!(
                "[gc] Major threshold set to {}",
                formatted_size(self.next_major_collection_threshold)
            );
        }
    }
    #[inline(always)]
    unsafe fn write_barrier_internal(&mut self, object: *mut HeapObjectHeader) {
        if (*self.old_space).has_address(object.cast())
            && (*object).is_precise()
            && (*PreciseAllocation::from_cell(object)).is_marked()
        {
            if !(*object).marked_bit() {
                (*object).set_marked_bit();
                self.write_barrier_slow(object);
            }
        }
    }

    #[cold]
    unsafe fn write_barrier_slow(&mut self, object: *mut HeapObjectHeader) {
        self.rem_set_lock.lock();
        self.remembered_set.push(object);
        self.rem_set_lock.unlock();
    }
}

#[cold]
fn promotion_oom(size: usize, object: *const u8) -> ! {
    eprintln!(
        "Out of memory on young spacep promotion: tried to promote {} at {:p}",
        formatted_size(size),
        object
    );
    std::process::abort()
}

pub struct YoungVisitor<'a> {
    serial: &'a mut Serial,
    parent_object: *mut HeapObjectHeader,
}

impl<'a> Visitor for YoungVisitor<'a> {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        unsafe {
            self.serial.trace_drag_out(root, self.parent_object);
        }
    }
}

pub struct OldVisitor<'a> {
    serial: &'a mut Serial,
}

impl<'a> Visitor for OldVisitor<'a> {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        unsafe {
            self.serial.trace(root);
        }
    }
}

impl GcBase for Serial {
    type TLAB = SimpleTLAB<Self>;
    const SUPPORTS_TLAB: bool = true;
    const LARGE_ALLOCATION_SIZE: usize = 16 * 1024;
    #[inline]
    fn write_barrier(&mut self, object: Gc<dyn crate::api::Collectable>) {
        unsafe {
            self.write_barrier_internal(object.base.as_ptr());
        }
    }

    fn collect_alloc_failure(&mut self, mutator: &MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        match SafepointScope::new(mutator.clone()) {
            Some(x) => unsafe {
                self.global_heap_lock.lock();
                self.rem_set_lock.lock();
                self.large_space_lock.lock();
                if self.minor(keep, GcReason::AllocationFailure) {
                    self.major(keep, GcReason::OldSpaceFull);
                }
                drop(x);
                self.global_heap_lock.unlock();
                self.rem_set_lock.unlock();
                self.large_space_lock.unlock();
            },
            None => return,
        }
    }
    fn collect(&mut self, mutator: &MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        unsafe {
            match SafepointScope::new(mutator.clone()) {
                Some(safepoint) => {
                    self.global_heap_lock.lock();
                    self.rem_set_lock.lock();
                    self.large_space_lock.lock();
                    if self.minor(keep, GcReason::RequestedByUser) {
                        self.major(keep, GcReason::OldSpaceFull);
                    }
                    drop(safepoint);
                    self.global_heap_lock.unlock();
                    self.rem_set_lock.unlock();
                    self.large_space_lock.unlock();
                }
                None => return,
            }
        }
    }
    fn full_collection(&mut self, mutator: &MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        unsafe {
            match SafepointScope::new(mutator.clone()) {
                Some(safepoint) => {
                    self.global_heap_lock.lock();
                    self.rem_set_lock.lock();
                    self.large_space_lock.lock();
                    self.minor(keep, GcReason::RequestedByUser);
                    self.major(keep, GcReason::RequestedByUser);

                    drop(safepoint);
                    self.global_heap_lock.unlock();
                    self.rem_set_lock.unlock();
                    self.large_space_lock.unlock();
                }
                None => return,
            }
        }
    }

    fn minor_collection(&mut self, mutator: &MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        unsafe {
            match SafepointScope::new(mutator.clone()) {
                Some(safepoint) => {
                    self.global_heap_lock.lock();
                    self.rem_set_lock.lock();
                    self.large_space_lock.lock();
                    self.minor(keep, GcReason::RequestedByUser);
                    drop(safepoint);
                    self.global_heap_lock.unlock();
                    self.rem_set_lock.unlock();
                    self.large_space_lock.unlock();
                }
                None => return,
            }
        }
    }

    fn alloc_inline<T: crate::api::Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        mut value: T,
    ) -> crate::api::Gc<T> {
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        let mut memory = self.nursery.bump_alloc(size);
        if memory.is_null() {
            self.collect_alloc_failure(mutator, &mut [&mut value]);

            //self.collect(mutator, &mut [&mut value]);
            memory = self.nursery.bump_alloc(size);
            if memory.is_null() {
                oom_abort();
            }
        }

        unsafe {
            let hdr = memory.cast::<HeapObjectHeader>();
            (*hdr).set_vtable(vtable_of::<T>());
            (*hdr).set_size(size);
            ((*hdr).data() as *mut T).write(value);
            Gc {
                base: NonNull::new_unchecked(hdr),
                marker: Default::default(),
            }
        }
    }
    fn alloc_tlab_area(&mut self, _mutator: &MutatorRef<Self>, _size: usize) -> *mut u8 {
        self.nursery.bump_alloc(32 * 1024)
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
    fn allocate_large<T: crate::api::Collectable + Sized + 'static>(
        &mut self,
        _mutator: &MutatorRef<Self>,
        value: T,
    ) -> crate::api::Gc<T> {
        unsafe {
            let size = value.allocation_size() + size_of::<HeapObjectHeader>();
            self.large_space_lock.lock();
            let object = self.large_space.allocate(size);
            (*object).set_vtable(vtable_of::<T>());
            (*object).type_id = small_type_id::<T>();
            let gc = Gc {
                base: NonNull::new_unchecked(object),
                marker: PhantomData::<T>,
            };
            ((*object).data() as *mut T).write(value);
            self.large_space_lock.unlock();
            self.post_alloc(gc);
            gc
        }
    }
}
