use std::{cell::Cell, sync::atomic::AtomicI32};

use parking_lot::{lock_api::RawMutex, Condvar, RawMutex as Mutex};

use crate::local_heap::LocalHeap;
use parking_lot::Mutex as GMutex;
/// Used to bring all threads with heap access to a safepoint such that e.g. a
/// garbage collection can be performed.
pub struct GlobalSafepoint {
    local_heaps_head: *mut LocalHeap,
    local_heaps_mutex: Mutex,
    barrier: Barrier,
    active_safepoint_scopes: AtomicI32,
}

struct Barrier {
    armed: Cell<bool>,
    mutex: GMutex<()>,
    cv_resume: Condvar,
    cv_stopped: Condvar,
    stopped: Cell<i32>,
}

impl Barrier {
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
}

impl GlobalSafepoint {
    pub fn enter_safepoint_scope(&self, stop_main_thread: bool) {
        if self
            .active_safepoint_scopes
            .fetch_add(1, atomic::Ordering::AcqRel)
            > 1
        {
            return;
        }
        self.local_heaps_mutex.lock();
        self.barrier.arm();
    }
}
