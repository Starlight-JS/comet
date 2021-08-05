use std::{cell::UnsafeCell, ptr::null_mut, sync::atomic::AtomicUsize};

use parking_lot::Mutex;

use crate::{
    global_allocator::GlobalAllocator,
    header::HeapObjectHeader,
    internal::{collection_barrier::CollectionBarrier, stack_bounds::StackBounds},
    large_space::PreciseAllocation,
    local_heap::LocalHeap,
    marking::SynchronousMarking,
    safepoint::GlobalSafepoint,
    task_scheduler::TaskScheduler,
    visitor::Visitor,
    Config,
};

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Copy)]
pub enum CollectionScope {
    Full,
    Eden,
}

pub trait MarkingConstraint {
    fn execute(&mut self, vis: &mut Visitor);
}

impl<T: FnMut(&mut Visitor)> MarkingConstraint for T {
    fn execute(&mut self, vis: &mut Visitor) {
        self(vis);
    }
}

#[allow(dead_code)]
pub struct Heap {
    safepoint: GlobalSafepoint,
    pub(crate) global: UnsafeCell<GlobalAllocator>,
    pub(crate) gc_prepare_stw_callback: Option<Box<dyn FnMut()>>,
    collection_barrier: CollectionBarrier,
    config: Config,
    pub(crate) task_scheduler: TaskScheduler,
    main_thread_local_heap: *mut LocalHeap,
    pub(crate) constraints: Mutex<Vec<Box<dyn MarkingConstraint>>>,
    pub(crate) current_visitor: Option<*mut Visitor>,
    generational_gc: bool,
    size_after_last_collect: usize,
    size_after_last_full_collect: usize,
    size_before_last_full_collect: usize,
    size_after_last_eden_collect: usize,
    size_before_last_eden_collect: usize,
    pub(crate) defers: AtomicUsize,
    pub(crate) bytes_allocated_this_cycle: usize,
    pub(crate) max_eden_size: usize,
    max_heap_size: usize,
    total_bytes_visited: usize,
    total_bytes_visited_this_cycle: usize,
    increment_balance: f64,
    should_do_full_collection: bool,
    collection_scope: Option<CollectionScope>,
    last_collection_scope: Option<CollectionScope>,
}

impl Heap {
    pub fn task_scheduler(&self) -> &TaskScheduler {
        &self.task_scheduler
    }

    pub fn add_constraint(&self, local: &LocalHeap, constraint: impl MarkingConstraint + 'static) {
        local.park();
        self.constraints.lock().push(Box::new(constraint));
        local.unpark();
    }

    pub fn add_core_constraints(&self, local: &LocalHeap) {
        self.add_constraint(local, |visitor: &mut Visitor| unsafe {
            let heap = &mut *visitor.heap();

            heap.global
                .get_mut()
                .large_space
                .prepare_for_conservative_scan();

            heap.safepoint().iterate(|local| {
                let mut from = (*local).bounds.origin;
                let mut to = (*local).last_sp.get();
                if to.is_null() {
                    return;
                }
                if from > to {
                    std::mem::swap(&mut to, &mut from);
                }

                visitor.trace_conservatively(from.cast(), to.cast());
            });
        });
    }

    pub fn visitor(&self) -> Option<&mut Visitor> {
        unsafe { self.current_visitor.map(|x| &mut *x) }
    }
    pub fn new(config: Config) -> (Box<Self>, Box<LocalHeap>) {
        let mut this = Box::new(Self {
            constraints: Mutex::new(Vec::new()),
            generational_gc: config.generational,
            safepoint: GlobalSafepoint::new(),
            defers: AtomicUsize::new(0),
            global: UnsafeCell::new(GlobalAllocator::new(&config)),
            gc_prepare_stw_callback: None,
            current_visitor: None,
            collection_barrier: CollectionBarrier::new(null_mut()),

            task_scheduler: TaskScheduler::new(),
            main_thread_local_heap: null_mut(),
            should_do_full_collection: false,
            size_after_last_collect: 0,
            size_after_last_eden_collect: 0,
            size_after_last_full_collect: 0,
            size_before_last_eden_collect: 0,
            size_before_last_full_collect: 0,
            max_eden_size: config.max_eden_size,
            max_heap_size: config.max_heap_size,
            collection_scope: None,
            last_collection_scope: None,
            total_bytes_visited: 0,
            total_bytes_visited_this_cycle: 0,
            increment_balance: 0.0,
            bytes_allocated_this_cycle: 0,
            config,
        });

        this.collection_barrier.heap = &mut *this;
        let mut local_heap = Box::new(LocalHeap::new(&mut this));
        local_heap.bounds = StackBounds::current_thread_stack_bounds();
        this.safepoint.add_local_heap(&mut *local_heap, || {});
        this.main_thread_local_heap = &mut *local_heap;
        local_heap.is_main = true;

        (this, local_heap)
    }

    pub fn spawn_background_thread<F, R>(
        &self,
        current_heap: &LocalHeap,
        callback: F,
    ) -> std::thread::JoinHandle<R>
    where
        F: FnOnce(&mut LocalHeap) -> R + Send + 'static,
        R: Send + 'static,
    {
        unsafe {
            current_heap.park();
            let mut heap = Box::new(LocalHeap::new(&mut *(self as *const Self as *mut Self)));
            self.safepoint.add_local_heap(&mut *heap, || {});
            let raw = Box::into_raw(heap) as usize;
            let handle = std::thread::spawn(move || {
                let mut heap = Box::from_raw(raw as *mut LocalHeap);
                heap.bounds = StackBounds::current_thread_stack_bounds();
                let result = callback(&mut heap);
                (*heap.heap).safepoint.remove_local_heap(&mut *heap, || {});
                result
            });

            current_heap.unpark();
            handle
        }
    }
    /// Wait for all the background threads to finish. Must be invoked only from the main thread only!
    pub fn join_all(&self) {
        self.safepoint.join_all();
    }
    pub unsafe fn main_thread_local_heap(&self) -> *mut LocalHeap {
        self.main_thread_local_heap
    }
    pub(crate) fn collection_barrier(&self) -> &CollectionBarrier {
        &self.collection_barrier
    }
    pub fn safepoint(&self) -> &GlobalSafepoint {
        &self.safepoint
    }

    pub(crate) unsafe fn sweep(&mut self) {
        // Sweep global allocator
        if let Some(CollectionScope::Full) = self.collection_scope {
            (*self.global.get()).sweep::<true>();
        } else {
            (*self.global.get()).sweep::<false>();
        }
    }

    pub(crate) fn collect_garbage(&mut self) {
        if self.defers.load(atomic::Ordering::SeqCst) > 0 {
            return;
        }
        self.perform_garbage_collection();
    }
    #[allow(dead_code)]
    pub(crate) fn collect_if_necessary_or_defer(&mut self) {
        if self.defers.load(atomic::Ordering::Relaxed) > 0 {
            return;
        } else {
            let bytes_allowed = self.max_eden_size;

            if as_atomic!(&self.bytes_allocated_this_cycle;AtomicUsize)
                .load(atomic::Ordering::Relaxed)
                >= bytes_allowed
            {
                self.collect_garbage();
            }
        }
    }
    fn will_start_collection(&mut self) {
        log_if!(self.config.verbose, " => ");
        if self.should_do_full_collection || !self.generational_gc {
            self.collection_scope = Some(CollectionScope::Full);
            self.should_do_full_collection = false;
            logln_if!(self.config.verbose, "FullCollection, ");
        } else {
            self.collection_scope = Some(CollectionScope::Eden);
            logln_if!(self.config.verbose, "EdenCollection ,");
        }
        if let Some(CollectionScope::Full) = self.collection_scope {
            self.size_before_last_full_collect =
                self.size_after_last_collect + self.bytes_allocated_this_cycle;
        } else {
            self.size_before_last_eden_collect =
                self.size_after_last_collect + self.bytes_allocated_this_cycle;
        }
    }
    fn update_object_counts(&mut self, bytes_visited: usize) {
        if let Some(CollectionScope::Full) = self.collection_scope {
            self.total_bytes_visited = 0;
        }
        self.total_bytes_visited_this_cycle = bytes_visited;
        self.total_bytes_visited += self.total_bytes_visited_this_cycle;
    }
    fn update_allocation_limits(&mut self) {
        // Calculate our current heap size threshold for the purpose of figuring out when we should
        // run another collection.
        let current_heap_size = self.total_bytes_visited;

        if let Some(CollectionScope::Full) = self.collection_scope {
            // To avoid pathological GC churn in very small and very large heaps, we set
            // the new allocation limit based on the current size of the heap, with a
            // fixed minimum.
            self.max_heap_size =
                (self.config.heap_growth_factor * current_heap_size as f64).ceil() as _;
            self.max_eden_size = self.max_heap_size - current_heap_size;
            self.size_after_last_full_collect = current_heap_size;
        } else {
            self.max_eden_size = if current_heap_size > self.max_heap_size {
                0
            } else {
                self.max_heap_size - current_heap_size
            };

            self.size_after_last_eden_collect = current_heap_size;
            let eden_to_old_gen_ratio = self.max_eden_size as f64 / self.max_heap_size as f64;
            let min_eden_to_old_gen_ratio = 1.0 / 3.0;
            logln_if!(
                self.config.verbose,
                " => Should perform full collection? {} = {} < {} ",
                eden_to_old_gen_ratio < min_eden_to_old_gen_ratio,
                eden_to_old_gen_ratio,
                min_eden_to_old_gen_ratio
            );
            if eden_to_old_gen_ratio < min_eden_to_old_gen_ratio {
                self.should_do_full_collection = true;
            }
            self.max_heap_size += current_heap_size - self.size_after_last_collect;
            self.max_eden_size = self.max_heap_size - current_heap_size;
        }
        self.size_after_last_collect = current_heap_size;
        self.bytes_allocated_this_cycle = 0;
        logln_if!(self.config.verbose, " => {}", current_heap_size);
    }
    pub(crate) fn test_and_set_marked(&self, hdr: *const HeapObjectHeader) -> bool {
        unsafe {
            if !(*hdr).is_precise() {
                (*self.global.get()).mark_bitmap.set(hdr as _)
            } else {
                (*PreciseAllocation::from_cell(hdr as _)).test_and_set_marked()
            }
        }
    }
    pub(crate) fn perform_garbage_collection(&mut self) {
        self.safepoint().enter_safepoint_scope(false);
        unsafe {
            self.safepoint().iterate(|local| {
                (*local).retain_blocks();
            });
            self.will_start_collection();
            self.global
                .get_mut()
                .prepare_for_marking(self.collection_scope == Some(CollectionScope::Eden));
            let live = {
                self.global
                    .get_mut()
                    .begin_marking(self.collection_scope != Some(CollectionScope::Eden));
                let mut marking = SynchronousMarking::new(self);
                marking.run()
            };

            self.sweep();
            self.update_object_counts(live);

            self.global
                .get_mut()
                .large_space
                .prepare_for_allocation(self.collection_scope == Some(CollectionScope::Eden));
            self.update_allocation_limits();
        }
        self.safepoint().leave_safepoint_scope(false);
    }
}

pub struct DeferPoint {
    defers: &'static AtomicUsize,
}
impl DeferPoint {
    pub fn new(local: &LocalHeap) -> Self {
        let this = Self {
            defers: as_atomic!(& { &*local.heap }.defers;AtomicUsize),
        };
        this.defers.fetch_add(1, atomic::Ordering::SeqCst);
        this
    }
}

impl Drop for DeferPoint {
    fn drop(&mut self) {
        self.defers.fetch_sub(1, atomic::Ordering::SeqCst);
    }
}
