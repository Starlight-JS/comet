use std::{cell::Cell, sync::atomic::AtomicBool};

use atomic::Atomic;
use crossbeam_utils::atomic::AtomicCell;
use parking_lot::{Condvar, Mutex};

use crate::{heap::Heap, local_heap::LocalHeap};

/// This structure stops and resumes all background threads waiting for GC.
pub struct CollectionBarrier {
    pub(crate) heap: *mut Heap,
    mutex: Mutex<()>,
    cv_wakeup: Condvar,
    collection_requested: AtomicBool,
    block_for_collection: Cell<bool>,
    shutdown_requested: Cell<bool>,
}

impl CollectionBarrier {
    pub fn new(heap: *mut Heap) -> Self {
        Self {
            heap,
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

    pub fn await_collection_background(&self, local_heap: *mut LocalHeap) -> bool {
        unsafe {
            let first_thread;
            {
                // Update flag before parking this thread, this guarantees that the flag is
                // set before the next GC.
                let guard = self.mutex.lock();
                if self.shutdown_requested.get() {
                    return false;
                }
                first_thread = !self.block_for_collection.get();
                self.block_for_collection.set(true);
                drop(guard);
            }
            let heap = (*local_heap).heap;
            if first_thread {
                if let Some(ref mut cb) = (*heap).gc_prepare_stw_callback {
                    cb();
                }
            }
            (*local_heap).park();
            let mut guard = self.mutex.lock();
            while self.block_for_collection.get() {
                if self.shutdown_requested.get() {
                    return false;
                }
                self.cv_wakeup.wait(&mut guard);
            }
            (*local_heap).unpark();

            true
        }
    }
}
