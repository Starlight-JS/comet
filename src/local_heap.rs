use std::{
    cell::Cell,
    ptr::{null_mut, NonNull},
};

use crate::{
    gcref::UntypedGcRef,
    global_allocator::{
        round_up, size_class_to_index, GlobalAllocator, LARGE_CUTOFF, NUM_SIZE_CLASSES,
    },
    header::HeapObjectHeader,
    heap::Heap,
    internal::{gc_info::GCInfoIndex, space_bitmap::SpaceBitmap, stack_bounds::StackBounds},
    local_allocator::LocalAllocator,
};
use atomic::Atomic;

/// LocalHeap is used by the GC to track all threads with heap access in order to
/// stop them before performing a collection. LocalHeaps can be either Parked or
/// Running and are in Parked mode when initialized.
///   Running: Thread is allowed to access the heap but needs to give the GC the
///            chance to run regularly by manually invoking Safepoint(). The
///            thread can be parked using ParkedScope.
///   Parked:  Heap access is not allowed, so the GC will not stop this thread
///            for a collection. Useful when threads do not need heap access for
///            some time or for blocking operations like locking a mutex.
pub struct LocalHeap {
    pub(crate) state: Atomic<ThreadState>,
    pub(crate) prev: *mut LocalHeap,
    pub(crate) next: *mut LocalHeap,
    pub(crate) is_main: bool,
    pub(crate) heap: *mut Heap,
    pub(crate) space_bitmap: *const SpaceBitmap<16>,
    pub(crate) global_heap: *mut GlobalAllocator,
    pub(crate) allocators: Box<[Option<LocalAllocator>]>,
    pub(crate) main_thread_parked: bool,
    pub(crate) bounds: StackBounds,
    pub(crate) last_sp: Cell<*mut u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ThreadState {
    /// Threads in this state are allowed to access the heap.
    Running,
    /// Thread was parked, which means that the thread is not allowed to access
    /// or manipulate the heap in any way. This is considered to be a safepoint.
    Parked,

    /// SafepointRequested is used for Running threads to force Safepoint() and
    /// Park() into the slow path.
    SafepointRequested,
    /// A thread transitions into this state from SafepointRequested when it
    /// enters a safepoint.
    Safepoint,
    /// This state is used for Parked background threads and forces Unpark() into
    /// the slow path. It prevents Unpark() to succeed before the safepoint
    /// operation is finished.
    ParkedSafepointRequested,
}

impl LocalHeap {
    pub(crate) fn new(heap: &mut Heap) -> Self {
        let global = heap.global.get();
        unsafe {
            let bitmap = &(*global).live_bitmap as *const _;
            Self {
                heap: heap as *mut _,
                global_heap: global,
                space_bitmap: bitmap,
                prev: null_mut(),
                next: null_mut(),
                main_thread_parked: false,
                bounds: StackBounds {
                    origin: null_mut(),
                    bound: null_mut(),
                },
                last_sp: Cell::new(null_mut()),
                allocators: vec![None; NUM_SIZE_CLASSES].into_boxed_slice(),
                is_main: false,
                state: Atomic::new(ThreadState::Running),
            }
        }
    }
    /// Allocates `size` bytes on the GC heap. If GC is required or no more memory left `None` is returned.
    #[allow(unused_unsafe)]
    pub unsafe fn allocate_raw(
        &mut self,
        gc_info: GCInfoIndex,
        size: usize,
    ) -> Option<UntypedGcRef> {
        self.safepoint();
        let size = round_up(size, 16);
        let mut is_large = false;
        let bytes = &(*self.heap).bytes_allocated_this_cycle;
        if as_atomic!(bytes;AtomicUsize).load(atomic::Ordering::Relaxed)
            >= (*self.heap).max_eden_size
        {
            self.try_perform_collection();
        };
        let (mem, size) = if size <= LARGE_CUTOFF {
            match self.allocators[size_class_to_index(size)] {
                Some(ref mut allocator) => allocator.allocate(),
                None => self.allocate_raw_small_slow(size),
            }
        } else {
            is_large = true;
            (*self.global_heap).large_allocation(size)
        };
        let bytes = &(*self.heap).bytes_allocated_this_cycle;
        as_atomic!(bytes;AtomicUsize).fetch_add(size, atomic::Ordering::Relaxed);
        if !mem.is_null() {
            let cell = mem.cast::<HeapObjectHeader>();
            (*cell).force_set_state(crate::header::CellState::DefinitelyWhite);
            (*cell).set_gc_info(gc_info);
            if !is_large {
                (*self.space_bitmap).set(cell.cast());
                (*cell).set_size(size);
            } else {
                (*cell).set_size(0);
            }
            Some(UntypedGcRef {
                header: NonNull::new_unchecked(cell),
            })
        } else {
            None
        }
    }
    /// Allocates `size` bytes on the heap. If no more memory is left or GC is required it requests a GC cycle.
    pub unsafe fn allocate_raw_or_fail(
        &mut self,
        gc_info: GCInfoIndex,
        size: usize,
    ) -> UntypedGcRef {
        let mem = self.allocate_raw(gc_info, size);
        if mem.is_none() {
            return self.perform_collection_and_allocate_again(gc_info, size);
        }

        mem.unwrap()
    }

    fn allocate_raw_small_slow(&mut self, size: usize) -> (*mut u8, usize) {
        self.init_allocator(size_class_to_index(size));
        self.allocators[size_class_to_index(size)]
            .as_mut()
            .unwrap()
            .allocate()
    }

    fn init_allocator(&mut self, index: usize) {
        self.allocators[index] = Some(LocalAllocator::new(
            self as *const Self as *mut Self,
            self.heap,
            self.global_heap,
            index,
        ));
    }
    #[cold]
    unsafe fn perform_collection_and_allocate_again(
        &mut self,
        gc_info: GCInfoIndex,
        size: usize,
    ) -> UntypedGcRef {
        for _ in 0..3 {
            if !self.try_perform_collection() {
                self.main_thread_parked = true;
            }
            let result = self.allocate_raw(gc_info, size);
            if result.is_some() {
                self.main_thread_parked = false;
                return result.unwrap();
            }
        }

        eprintln!("LocalHeap: allocation failed");
        std::process::abort();
    }

    pub fn safepoint(&self) -> bool {
        let current = self.state.load(atomic::Ordering::Relaxed);
        if current == ThreadState::SafepointRequested {
            self.safepoint_slow_path();
            true
        } else {
            false
        }
    }

    pub(crate) fn retain_blocks(&mut self) {
        for alloc in self
            .allocators
            .iter_mut()
            .filter(|x| x.is_some())
            .map(|x| x.as_mut().unwrap())
        {
            if !alloc.current_block.is_null() {
                alloc.unavailable.push(alloc.current_block);
                alloc.current_block = null_mut();
            }

            while !alloc.unavailable.is_empty() {
                let block = alloc.unavailable.pop();
                unsafe {
                    (*self.global_heap).free_blocks[size_class_to_index(alloc.cell_size as _)]
                        .add_free(block);
                }
            }
        }
    }
    /*
    pub(crate) fn sweep(&mut self) -> usize {
        let mut nblocks = 0;
        for alloc in self
            .allocators
            .iter_mut()
            .filter(|x| x.is_some())
            .map(|x| x.as_mut().unwrap())
        {
            if !alloc.current_block.is_null() {
                alloc.unavailable.push(alloc.current_block);
            }

            while !alloc.unavailable.is_empty() {
                let block = alloc.unavailable.pop();
                println!("sweep block (local) {:p}", block);
                unsafe {
                    match (*block).sweep(&(*self.global_heap).live_bitmap) {
                        crate::block::SweepResult::Empty => {
                            (*self.global_heap).block_allocator.return_block(block);
                        }
                        crate::block::SweepResult::Full => {
                            nblocks += 1;
                            (*self.global_heap).unavail_blocks
                                [size_class_to_index(alloc.cell_size as _)]
                            .add_free(block);
                        }
                        crate::block::SweepResult::Reusable => {
                            nblocks += 1;
                            (*self.global_heap).free_blocks
                                [size_class_to_index(alloc.cell_size as _)]
                            .add_free(block);
                        }
                    }
                }
            }
        }
        nblocks
    }*/
    pub fn is_parked(&self) -> bool {
        let state = self.state.load(atomic::Ordering::Relaxed);
        state == ThreadState::Parked || state == ThreadState::ParkedSafepointRequested
    }
    fn park_slow_path(&self, mut current_state: ThreadState) {
        if self.is_main {
            loop {
                assert_eq!(current_state, ThreadState::ParkedSafepointRequested);
                unsafe {
                    (*self.heap).collect_garbage();
                }
                current_state = ThreadState::Running;
                if self
                    .state
                    .compare_exchange(
                        current_state,
                        ThreadState::Parked,
                        atomic::Ordering::SeqCst,
                        atomic::Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return;
                }
            }
        } else {
            unsafe {
                (*self.heap).safepoint().notify_park();
            }
        }
    }

    fn unpark_slow_path(&self) {
        if self.is_main {
            let expected = ThreadState::ParkedSafepointRequested;
            assert!(self
                .state
                .compare_exchange(
                    expected,
                    ThreadState::SafepointRequested,
                    atomic::Ordering::SeqCst,
                    atomic::Ordering::Relaxed,
                )
                .is_ok());
        } else {
            loop {
                let expected = ThreadState::Parked;
                if !self
                    .state
                    .compare_exchange(
                        expected,
                        ThreadState::Running,
                        atomic::Ordering::SeqCst,
                        atomic::Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    unsafe {
                        (*self.heap).safepoint().wait_in_unpark();
                    }
                } else {
                    return;
                }
            }
        }
    }
    #[cold]
    fn safepoint_slow_path(&self) {
        if self.is_main {
            unsafe {
                self.last_sp.set(approximate_stack_pointer());
                (*self.heap).collect_garbage();
            }
        } else {
            let expected = ThreadState::SafepointRequested;
            assert!(self
                .state
                .compare_exchange(
                    expected,
                    ThreadState::Safepoint,
                    atomic::Ordering::SeqCst,
                    atomic::Ordering::Relaxed
                )
                .is_ok());
            self.last_sp.set(approximate_stack_pointer());
            unsafe {
                (*self.heap).safepoint().wait_in_safepoint();
            }

            self.unpark();
        }
    }
    pub fn unpark(&self) {
        let expected = ThreadState::Parked;
        if self
            .state
            .compare_exchange(
                expected,
                ThreadState::Running,
                atomic::Ordering::SeqCst,
                atomic::Ordering::Relaxed,
            )
            .is_err()
        {
            self.unpark_slow_path();
        }
    }

    pub fn park(&self) {
        let expected = ThreadState::Running;
        if self
            .state
            .compare_exchange(
                expected,
                ThreadState::Parked,
                atomic::Ordering::SeqCst,
                atomic::Ordering::Relaxed,
            )
            .is_err()
        {
            self.park_slow_path(expected);
        }
    }

    pub fn try_perform_collection(&self) -> bool {
        unsafe {
            self.last_sp.set(approximate_stack_pointer());
            if self.is_main {
                (*self.heap).collect_garbage();
                return true;
            } else {
                (*self.heap).collection_barrier().request_gc();
                let main_thread = (*self.heap).main_thread_local_heap();
                let current = (*main_thread).state.load(atomic::Ordering::Relaxed);
                loop {
                    match current {
                        ThreadState::Running => {
                            if (*main_thread)
                                .state
                                .compare_exchange(
                                    current,
                                    ThreadState::SafepointRequested,
                                    atomic::Ordering::SeqCst,
                                    atomic::Ordering::Relaxed,
                                )
                                .is_ok()
                            {
                                return (*self.heap)
                                    .collection_barrier()
                                    .await_collection_background(self as *const Self as _);
                            }
                        }
                        ThreadState::SafepointRequested => {
                            return (*self.heap)
                                .collection_barrier()
                                .await_collection_background(self as *const Self as _);
                        }
                        ThreadState::Parked => {
                            if (*main_thread)
                                .state
                                .compare_exchange(
                                    current,
                                    ThreadState::ParkedSafepointRequested,
                                    atomic::Ordering::SeqCst,
                                    atomic::Ordering::Relaxed,
                                )
                                .is_ok()
                            {
                                return false;
                            }
                        }
                        ThreadState::ParkedSafepointRequested => return false,
                        ThreadState::Safepoint => unreachable!(),
                    }
                }
            }
        }
    }
}
#[inline(always)]
pub fn approximate_stack_pointer() -> *mut u8 {
    let mut result = null_mut();
    result = &mut result as *mut *mut u8 as *mut u8;
    result
}
