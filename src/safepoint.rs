use std::{
    cell::{Cell, UnsafeCell},
    ptr::null_mut,
    sync::atomic::AtomicI32,
};

use parking_lot::{Condvar, Mutex};

use crate::local_heap::{LocalHeap, ThreadState};
use parking_lot::Mutex as GMutex;
/// Used to bring all threads with heap access to a safepoint such that e.g. a
/// garbage collection can be performed.
pub struct GlobalSafepoint {
    local_heaps_head: UnsafeCell<*mut LocalHeap>,
    local_heaps_mutex: Mutex<u32>,
    barrier: Barrier,
    active_safepoint_scopes: AtomicI32,
    cv_join: Condvar,
}

struct Barrier {
    armed: Cell<bool>,
    mutex: GMutex<()>,
    cv_resume: Condvar,
    cv_stopped: Condvar,
    stopped: Cell<i32>,
}

impl Barrier {
    pub fn new() -> Self {
        Self {
            armed: Cell::new(false),
            mutex: GMutex::new(()),
            cv_resume: Condvar::new(),
            cv_stopped: Condvar::new(),
            stopped: Cell::new(0),
        }
    }
    pub fn is_armed(&self) -> bool {
        self.armed.get()
    }

    pub fn arm(&self) {
        let l = self.mutex.lock();
        debug_assert!(!self.is_armed());
        self.armed.set(true);
        self.stopped.set(0);
        drop(l);
    }
    pub fn disarm(&self) {
        let l = self.mutex.lock();
        self.armed.set(false);
        self.stopped.set(0);
        self.cv_resume.notify_all();
        drop(l);
    }

    pub fn wait_until_running_threads_in_safepoint(&self, running: i32) {
        let mut guard = self.mutex.lock();
        while self.stopped.get() < running {
            self.cv_stopped.wait(&mut guard);
        }

        debug_assert_eq!(self.stopped.get(), running);
    }

    pub fn notify_park(&self) {
        let guard = self.mutex.lock();
        self.stopped.set(self.stopped.get() + 1);
        self.cv_stopped.notify_one();
        drop(guard);
    }
    pub fn wait_in_safepoint(&self) {
        let mut guard = self.mutex.lock();
        self.stopped.set(self.stopped.get() + 1);
        self.cv_stopped.notify_one();
        while self.is_armed() {
            self.cv_resume.wait(&mut guard);
        }
    }

    pub fn wait_in_unpark(&self) {
        let mut guard = self.mutex.lock();
        while self.is_armed() {
            self.cv_resume.wait(&mut guard);
        }
    }
}

impl GlobalSafepoint {
    pub fn new() -> Self {
        Self {
            barrier: Barrier::new(),
            local_heaps_head: UnsafeCell::new(null_mut()),
            local_heaps_mutex: Mutex::new(0),
            cv_join: Condvar::new(),
            active_safepoint_scopes: AtomicI32::new(0),
        }
    }
    pub fn wait_in_safepoint(&self) {
        self.barrier.wait_in_safepoint();
    }

    pub fn wait_in_unpark(&self) {
        self.barrier.wait_in_unpark();
    }

    pub fn notify_park(&self) {
        self.barrier.notify_park();
    }

    pub fn iterate(&self, mut f: impl FnMut(*mut LocalHeap)) {
        unsafe {
            let g = self.local_heaps_mutex.lock();
            let mut cur = *self.local_heaps_head.get();
            while !cur.is_null() {
                f(cur);
                cur = (*cur).next;
            }
            drop(g);
        }
    }
    pub fn contains_local_heap(&self, local_heap: *mut LocalHeap) -> bool {
        unsafe {
            let g = self.local_heaps_mutex.lock();
            let mut cur = *self.local_heaps_head.get();
            while !cur.is_null() {
                if cur == local_heap {
                    drop(g);
                    return true;
                }
                cur = (*cur).next;
            }
            drop(g);
            false
        }
    }
    pub fn contains_any_local_heap(&self) -> bool {
        let g = self.local_heaps_mutex.lock();
        let res = unsafe { !(*self.local_heaps_head.get()).is_null() };
        drop(g);
        res
    }
    pub fn enter_safepoint_scope(&self, stop_main_thread: bool) {
        if self
            .active_safepoint_scopes
            .fetch_add(1, atomic::Ordering::AcqRel)
            + 1
            > 1
        {
            return;
        }
        let g = self.local_heaps_mutex.lock();
        self.barrier.arm();
        std::mem::forget(g);
        let mut running = 0;
        unsafe {
            let mut local_heap = *self.local_heaps_head.get();
            while !local_heap.is_null() {
                if (*local_heap).is_main && !stop_main_thread {
                    continue;
                }
                let expected = (*local_heap).state.load(atomic::Ordering::Relaxed);
                loop {
                    let new_state = if expected == ThreadState::Parked {
                        ThreadState::ParkedSafepointRequested
                    } else {
                        ThreadState::SafepointRequested
                    };

                    if (*local_heap)
                        .state
                        .compare_exchange(
                            expected,
                            new_state,
                            atomic::Ordering::SeqCst,
                            atomic::Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        if expected == ThreadState::Running {
                            running += 1;
                        } else {
                            assert_eq!(expected, ThreadState::Parked);
                        }
                        break;
                    }
                }
                local_heap = (*local_heap).next;
            }
        }
        self.barrier
            .wait_until_running_threads_in_safepoint(running);
    }

    pub fn leave_safepoint_scope(&self, stop_main_thread: bool) {
        unsafe {
            if self
                .active_safepoint_scopes
                .fetch_sub(1, atomic::Ordering::AcqRel)
                - 1
                > 0
            {
                return;
            }
            let mut local_heap = *self.local_heaps_head.get();
            while !local_heap.is_null() {
                if (*local_heap).is_main && !stop_main_thread {
                    continue;
                }
                // We transition both ParkedSafepointRequested and Safepoint states to
                // Parked. While this is probably intuitive for ParkedSafepointRequested,
                // this might be surprising for Safepoint though. SafepointSlowPath() will
                // later unpark that thread again. Going through Parked means that a
                // background thread doesn't need to be waked up before the main thread can
                // start the next safepoint.
                let old_state = (*local_heap)
                    .state
                    .swap(ThreadState::Parked, atomic::Ordering::AcqRel);
                assert!(
                    old_state == ThreadState::ParkedSafepointRequested
                        || old_state == ThreadState::Safepoint
                );
                local_heap = (*local_heap).next;
            }
            self.barrier.disarm();
            self.local_heaps_mutex.force_unlock();
        }
    }

    pub fn add_local_heap(&self, local_heap: *mut LocalHeap, callback: impl FnOnce()) {
        let mut g = self.local_heaps_mutex.lock();
        unsafe {
            // Additional code protected from safepoint
            callback();
            let head = &mut *self.local_heaps_head.get();
            // Add list to doubly-linked list
            if !head.is_null() {
                (**head).prev = local_heap;
            }
            (*local_heap).prev = null_mut();
            (*local_heap).next = *head;
            *head = local_heap;
            *g += 1;
            drop(g);
        }
    }
    pub fn remove_local_heap(&self, local_heap: *mut LocalHeap, callback: impl FnOnce()) {
        let mut g = self.local_heaps_mutex.lock();
        unsafe {
            // Additional code protected from safepoint
            callback();
            let head = &mut *self.local_heaps_head.get();
            // Remove list from doubly-linked list
            if !(*local_heap).next.is_null() {
                (*(*local_heap).next).prev = (*local_heap).prev;
            }
            if !(*local_heap).prev.is_null() {
                (*(*local_heap).prev).next = (*local_heap).next;
            } else {
                *head = (*local_heap).next;
            }
            *g -= 1;
            self.cv_join.notify_all();
            drop(g);
        }
    }
    /// Wait for all the background threads to finish.
    pub fn join_all(&self) {
        let mut lock = self.local_heaps_mutex.lock();
        // There's always one main thread.
        while *lock > 1 {
            self.cv_join.wait(&mut lock);
        }
    }
}

unsafe impl Send for GlobalSafepoint {}
unsafe impl Sync for GlobalSafepoint {}
