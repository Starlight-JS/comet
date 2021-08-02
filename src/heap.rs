use std::{cell::UnsafeCell, ptr::null_mut};

use crate::{
    global_allocator::GlobalAllocator, internal::collection_barrier::CollectionBarrier,
    local_heap::LocalHeap, safepoint::GlobalSafepoint, Config,
};

pub struct Heap {
    safepoint: GlobalSafepoint,
    pub(crate) global: UnsafeCell<GlobalAllocator>,
    pub(crate) gc_prepare_stw_callback: Option<Box<dyn FnMut()>>,
    collection_barrier: CollectionBarrier,
    config: Config,
    main_thread_local_heap: *mut LocalHeap,
}

impl Heap {
    pub fn new(config: Config) -> (Box<Self>, Box<LocalHeap>) {
        let mut this = Box::new(Self {
            safepoint: GlobalSafepoint::new(),
            global: UnsafeCell::new(GlobalAllocator::new(&config)),
            gc_prepare_stw_callback: None,
            collection_barrier: CollectionBarrier::new(null_mut()),
            config,
            main_thread_local_heap: null_mut(),
        });

        this.collection_barrier.heap = &mut *this;
        let mut local_heap = Box::new(LocalHeap::new(&mut this));
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

    pub(crate) unsafe fn sweep(&self) {
        // Sweep global allocator
        (*self.global.get()).sweep();
        // Sweep local allocators.
        self.safepoint().iterate(|local| unsafe {
            (*local).sweep();
        });
    }

    pub(crate) fn collect_garbage(&mut self) {}
}
