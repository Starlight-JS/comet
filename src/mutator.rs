use std::{
    cell::{Cell, UnsafeCell},
    mem::size_of,
    ops::{Deref, DerefMut},
    ptr::{null_mut, NonNull},
    sync::{atomic::AtomicU32, Arc},
};

use atomic::{Atomic, Ordering};
use parking_lot::{Condvar, Mutex};

use crate::{
    api::{Collectable, Finalize, Gc, HeapObjectHeader, Trace},
    gc_base::{GcBase, TLAB},
    safepoint::GlobalSafepoint,
    shadow_stack::ShadowStack,
    utils::align_usize,
};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ThreadState {
    Unsafe = 0,
    Waiting = 1,
    Safe = 2,
}

impl ThreadState {
    pub fn safe_for_safepoint(self) -> bool {
        match self {
            Self::Unsafe | Self::Waiting => true,
            _ => false,
        }
    }
}

pub struct Mutator<H: GcBase + 'static> {
    pub(crate) tlab: H::TLAB,

    pub(crate) state: Atomic<ThreadState>,

    safepoint: *const GlobalSafepoint,
    safepoint_cond: *const AtomicU32,
    last_sp: Cell<*mut *mut u8>,
    join_data: Arc<JoinDataInternal>,
    shadow_stack: ShadowStack,
    pub(crate) heap: Arc<UnsafeCell<H>>,
    rc: u32,
}

impl<H: 'static + GcBase> Mutator<H> {
    pub unsafe fn reset_tlab(&mut self) {
        self.tlab.reset();
    }
    /// Spawn mutator thread attached to the heap.
    pub fn spawn_mutator<F>(&self, closure: F) -> JoinData
    where
        F: FnOnce(MutatorRef<H>) + Send + 'static,
    {
        let state = self.enter_unsafe();
        let heap = self.heap_ref();
        let join_data = JoinData::new();
        let mut mutator = MutatorRef::new(Mutator::new(
            self.heap.clone(),
            heap.safepoint(),
            join_data.internal.clone(),
        ));

        heap.attach_current_thread(&mut *mutator);
        drop(state);
        std::thread::spawn(move || {
            mutator.state_set(ThreadState::Safe, ThreadState::Unsafe);
            closure(mutator.clone());
            mutator.stop();
            drop(mutator);
        });

        join_data
    }
    pub(crate) fn heap_ref(&self) -> &mut H {
        unsafe { &mut *self.heap.get() }
    }
    pub(crate) fn new(
        heap: Arc<UnsafeCell<H>>,
        safepoint: *const GlobalSafepoint,
        join_data: Arc<JoinDataInternal>,
    ) -> Mutator<H> {
        Mutator {
            heap: heap.clone(),
            safepoint,
            safepoint_cond: unsafe { &(&*safepoint).gc_running },
            state: Atomic::new(ThreadState::Unsafe),
            tlab: H::TLAB::create(heap),
            last_sp: Cell::new(null_mut()),
            join_data,
            shadow_stack: ShadowStack::new(),
            rc: 1,
        }
    }

    pub fn shadow_stack(&self) -> &'static ShadowStack {
        unsafe { std::mem::transmute(&self.shadow_stack) }
    }

    fn get_safepoint(&self) -> &GlobalSafepoint {
        unsafe { &*self.safepoint }
    }

    pub(crate) fn set_gc_and_wait(&self) {
        let state = self.state.load(Ordering::Relaxed);

        self.state.store(ThreadState::Waiting, Ordering::Release);
        self.get_safepoint().wait_gc();
        self.state.store(state, Ordering::Release);
    }

    #[inline(always)]
    pub fn safepoint(&self) -> bool {
        unsafe {
            if (*self.safepoint_cond).load(Ordering::Relaxed) != 0 {
                self.safepoint_slow();
                return true;
            }
            false
        }
    }
    #[inline(never)]
    #[cold]
    fn safepoint_slow(&self) {
        self.last_sp.set(approximate_stack_pointer());
        self.set_gc_and_wait();
    }

    pub(crate) fn state_set(&self, state: ThreadState, old_state: ThreadState) -> ThreadState {
        self.last_sp.set(approximate_stack_pointer());
        self.state.store(state, Ordering::Release);

        if old_state.safe_for_safepoint() && !state.safe_for_safepoint() {
            self.safepoint();
        }
        old_state
    }

    pub(crate) fn state_save_and_set(&self, state: ThreadState) -> ThreadState {
        self.state_set(state, self.state.load(Ordering::Relaxed))
    }

    pub fn enter_unsafe(&self) -> UnsafeMutatorState<H> {
        let state = self.state_save_and_set(ThreadState::Unsafe);
        UnsafeMutatorState {
            mutator: self as *const Self,
            gc_state: state,
        }
    }

    pub fn enter_safe(&self) -> UnsafeMutatorState<H> {
        let state = self.state_save_and_set(ThreadState::Safe);
        UnsafeMutatorState {
            mutator: self as *const Self,
            gc_state: state,
        }
    }

    pub(crate) fn stop(&self) {
        let mut running = (&*self.join_data).running.lock();
        *running = false;
        (&*self.join_data).cv_stopped.notify_all();
    }
}

impl<H: GcBase> MutatorRef<H> {
    pub fn write_barrier(&self, object: Gc<dyn Collectable>) {
        self.heap_ref().write_barrier(object);
    }
    pub fn collect(&self, keep: &mut [&mut dyn Trace]) {
        self.heap_ref().collect(self, keep);
    }

    #[inline(always)]
    pub unsafe fn allocate_from_tlab<T: Collectable + Sized + 'static>(
        &mut self,
        value: T,
    ) -> Result<Gc<T>, T> {
        self.tlab.allocate(value)
    }
    /// Allocate `T` on GC heap
    #[inline(always)]
    pub fn allocate<T: Collectable + Sized + 'static>(&mut self, value: T) -> Gc<T> {
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        if !self.tlab.can_thread_local_allocate(size) && size >= H::LARGE_ALLOCATION_SIZE {
            return self.allocate_slow(value, size);
        } else if !self.tlab.can_thread_local_allocate(size) || !H::SUPPORTS_TLAB {
            return self.allocate_inline(value, size);
        }

        let result = self.tlab.allocate(value);

        match result {
            Ok(value) => value,
            Err(value) => self.allocate_slow(value, size),
        }
    }

    #[cold]
    fn allocate_slow<T: Collectable + Sized + 'static>(
        &mut self,
        mut value: T,
        size: usize,
    ) -> Gc<T> {
        if size >= H::LARGE_ALLOCATION_SIZE {
            self.heap_ref().allocate_large(self, value)
        } else if self.tlab.can_thread_local_allocate(size) && H::SUPPORTS_TLAB {
            // try to refill tlab if gc supports tlab
            let mut this = self.clone();
            if !this.tlab.refill(&self.clone(), size) {
                // if tlab failed to be refilled we request GC cycle and try to get some memory
                self.heap_ref()
                    .collect_alloc_failure(self, &mut [&mut value]);
                if !this.tlab.refill(&self, size) {
                    // if refilling again fails we just OOM
                    oom_abort();
                }
            }
            // must not fail
            self.allocate(value)
        } else {
            // this path should be reached only when `H::SUPPORTS_TLAB` returns true and `size` is `>= H::TLAB::LARGE_OBJECT_SIZE`
            self.allocate_inline(value, size)
        }
    }

    /// Invoked when `H::SUPPORTS_TLAB` returns false or when allocation size is larger than [TLAB::TLAB_OBJET_SIZE] but smaller than `H::LARGE_ALLOCATION_SIZE`.
    ///
    /// Performance of this function depends only on GC implementation of [GcBase::alloc_inline]
    #[inline(always)]
    fn allocate_inline<T: Collectable + Sized + 'static>(
        &mut self,
        value: T,
        _size: usize,
    ) -> Gc<T> {
        let href = unsafe { &mut *self.heap.get() };
        let val = href.alloc_inline(self, value);
        val
    }
}

#[inline(always)]
fn approximate_stack_pointer() -> *mut *mut u8 {
    let mut result = null_mut();
    result = &mut result as *mut *mut *mut u8 as *mut *mut u8;
    result
}

pub struct UnsafeMutatorState<H: 'static + GcBase> {
    mutator: *const Mutator<H>,
    gc_state: ThreadState,
}

impl<H: 'static + GcBase> Drop for UnsafeMutatorState<H> {
    fn drop(&mut self) {
        unsafe {
            (&*self.mutator).state_save_and_set(self.gc_state);
        }
    }
}

pub struct SafeMutatorState<H: 'static + GcBase> {
    mutator: *const Mutator<H>,
    gc_state: ThreadState,
}

impl<H: 'static + GcBase> Drop for SafeMutatorState<H> {
    fn drop(&mut self) {
        unsafe {
            (&*self.mutator).state_save_and_set(self.gc_state);
        }
    }
}

pub(crate) struct JoinDataInternal {
    running: Mutex<bool>,
    cv_stopped: Condvar,
}

impl JoinDataInternal {
    fn new() -> JoinDataInternal {
        JoinDataInternal {
            running: Mutex::new(true),
            cv_stopped: Condvar::new(),
        }
    }
}

pub struct JoinData {
    pub(crate) internal: Arc<JoinDataInternal>,
}

impl JoinData {
    pub(crate) fn new() -> Self {
        Self {
            internal: Arc::new(JoinDataInternal::new()),
        }
    }
    pub fn join<H: 'static + GcBase>(self, mutator: &Mutator<H>) {
        let state = mutator.enter_unsafe();
        let mut running = self.internal.running.lock();

        while *running {
            self.internal.cv_stopped.wait(&mut running);
        }

        drop(state);
    }
}

unsafe impl<H: GcBase> Trace for Mutator<H> {}
unsafe impl<H: GcBase> Finalize for Mutator<H> {}
unsafe impl<H: GcBase> Send for Mutator<H> {}

impl<H: 'static + GcBase> Drop for Mutator<H> {
    fn drop(&mut self) {
        let mutator = self;
        let mptr = mutator as *mut Self;
        let state = mutator.enter_unsafe();

        let heap = mutator.heap_ref();

        heap.detach_current_thread(mptr);
        mutator.stop();
        drop(state);
    }
}

pub struct MutatorRef<H: GcBase + 'static> {
    mutator: NonNull<Mutator<H>>,
}

impl<H: GcBase + 'static> MutatorRef<H> {
    pub fn new(mutator: Mutator<H>) -> Self {
        Self {
            mutator: unsafe { NonNull::new_unchecked(Box::into_raw(Box::new(mutator))) },
        }
    }
}

impl<H: GcBase + 'static> Deref for MutatorRef<H> {
    type Target = Mutator<H>;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutator.as_ptr() }
    }
}

impl<H: GcBase + 'static> DerefMut for MutatorRef<H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutator.as_ptr() }
    }
}

impl<H: GcBase + 'static> Clone for MutatorRef<H> {
    fn clone(&self) -> Self {
        unsafe {
            (&mut *self.mutator.as_ptr()).rc += 1;
            Self {
                mutator: self.mutator,
            }
        }
    }
}

impl<H: GcBase + 'static> Drop for MutatorRef<H> {
    fn drop(&mut self) {
        unsafe {
            (*self.mutator.as_ptr()).rc -= 1;
            if (*self.mutator.as_ptr()).rc == 0 {
                core::ptr::drop_in_place(self.mutator.as_ptr());
            }
        }
    }
}

unsafe impl<H: GcBase + 'static> Send for MutatorRef<H> {}

#[cold]
pub fn oom_abort() -> ! {
    eprintln!("OutOfMemory");
    std::process::abort();
}
