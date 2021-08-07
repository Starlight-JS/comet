use parking_lot::{Condvar, Mutex};
use std::{cell::Cell, sync::atomic::AtomicBool};

use crate::heap::Heap;

/// This structure stops and resumes all background threads waiting for GC.
pub struct CollectionBarrier {
    mutex: Mutex<()>,
    cv_wakeup: Condvar,
    collection_requested: AtomicBool,
    block_for_collection: Cell<bool>,
    shutdown_requested: Cell<bool>,
}

impl CollectionBarrier {
    pub fn new(_heap: *mut Heap) -> Self {
        Self {
            mutex: Mutex::new(()),
            collection_requested: AtomicBool::new(false),
            cv_wakeup: Condvar::new(),
            block_for_collection: Cell::new(false),
            shutdown_requested: Cell::new(false),
        }
    }

    pub fn was_gc_requested(&self) -> bool {
        self.collection_requested.load(atomic::Ordering::Relaxed)
    }
    pub fn request_gc(&self) {
        let guard = self.mutex.lock();
        let was_already_requested = self
            .collection_requested
            .swap(true, atomic::Ordering::AcqRel);
        let _ = was_already_requested;
        drop(guard);
    }

    pub fn notify_shutdown_requested(&self) {
        let guard = self.mutex.lock();
        self.shutdown_requested.set(true);
        self.cv_wakeup.notify_all();
        drop(guard);
    }
    pub fn resume_threads_awaiting_collection(&self) {
        let guard = self.mutex.lock();
        self.collection_requested
            .store(false, atomic::Ordering::Release);
        self.block_for_collection.set(false);
        self.cv_wakeup.notify_all();
        drop(guard);
    }
}
