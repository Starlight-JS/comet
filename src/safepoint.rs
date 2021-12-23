use std::{
    cell::{Cell, UnsafeCell},
    sync::{atomic::AtomicU32, Arc},
};

use atomic::Ordering;
use parking_lot::{lock_api::RawMutex, RawMutex as Lock};

use crate::{
    gc_base::GcBase,
    mutator::{MutatorRef, ThreadState},
};

pub struct GlobalSafepoint {
    pub(crate) safepoint_lock: Lock,
    pub(crate) safepoint_enable_cnt: Cell<u8>,
    pub(crate) gc_running: AtomicU32,
    pub(crate) n_mutators: AtomicU32,
}

impl GlobalSafepoint {
    pub(crate) fn new() -> Self {
        Self {
            safepoint_enable_cnt: Cell::new(0),
            safepoint_lock: Lock::INIT,
            gc_running: AtomicU32::new(0),
            n_mutators: AtomicU32::new(0),
        }
    }
    fn enable(&self) {
        debug_assert!(self.safepoint_lock.is_locked());
        self.safepoint_enable_cnt
            .set(self.safepoint_enable_cnt.get() + 1);
    }

    fn disable(&self) {
        debug_assert!(self.safepoint_lock.is_locked());
        self.safepoint_enable_cnt
            .set(self.safepoint_enable_cnt.get() - 1);
    }

    pub fn start(&self) -> bool {
        if self.n_mutators.load(atomic::Ordering::Relaxed) == 1 {
            self.gc_running.store(1, atomic::Ordering::Relaxed);
            return true;
        }
        self.safepoint_lock.lock();
        let running = 0;
        // In case multiple threads enter the GC at the same time, only allow
        // one of them to actually run the collection. We can't just let the
        // master thread do the GC since it might be running unmanaged code
        // and can take arbitrarily long time before hitting a safe point.
        if let Err(_) = self.gc_running.compare_exchange_weak(
            running,
            1,
            atomic::Ordering::AcqRel,
            atomic::Ordering::Relaxed,
        ) {
            unsafe {
                self.safepoint_lock.unlock();
                self.wait_gc();
                return false;
            }
        }
        assert!(self.gc_running.load(Ordering::Relaxed) == 1);

        self.enable();
        unsafe {
            self.safepoint_lock.unlock();
        }
        true
    }

    pub fn end(&self) {
        if self.n_mutators.load(atomic::Ordering::Relaxed) == 1 {
            self.gc_running.store(0, atomic::Ordering::Relaxed);
            return;
        }

        self.safepoint_lock.lock();

        self.disable();
        self.gc_running.store(0, atomic::Ordering::Release);
        unsafe {
            self.safepoint_lock.unlock();
        }
    }
    #[inline]
    pub fn wait_gc(&self) {
        while self.gc_running.load(atomic::Ordering::Relaxed) != 0
            || self.gc_running.load(atomic::Ordering::Acquire) != 0
        {
            std::hint::spin_loop();
        }
    }
}

pub struct SafepointScope<H: 'static + GcBase> {
    heap: Arc<UnsafeCell<H>>,
    old_state: ThreadState,
    mutator: MutatorRef<H>,
}

impl<H: 'static + GcBase> SafepointScope<H> {
    pub fn new(mutator: MutatorRef<H>) -> Option<Self> {
        let href = unsafe { &*mutator.heap.get() };
        let safepoint = href.safepoint();
        let old_state = mutator.state.load(Ordering::Relaxed);
        mutator
            .state
            .store(crate::mutator::ThreadState::Waiting, Ordering::Release);
        if !safepoint.start() {
            mutator.state_set(old_state, crate::mutator::ThreadState::Waiting);
            return None;
        }

        let this = Self {
            heap: mutator.heap.clone(),
            old_state,
            mutator,
        };

        unsafe {
            let href = &mut *this.heap.get();

            href.global_lock();
            let mutators = href.mutators();

            for mutator in mutators {
                while !(**mutator)
                    .state
                    .load(Ordering::Relaxed)
                    .safe_for_safepoint()
                    || !(**mutator)
                        .state
                        .load(Ordering::Acquire)
                        .safe_for_safepoint()
                {
                    std::hint::spin_loop();
                }
            }

            href.global_unlock();
        }
        Some(this)
    }

    pub fn heap(&self) -> Arc<UnsafeCell<H>> {
        return self.heap.clone();
    }
}

impl<H: GcBase> Drop for SafepointScope<H> {
    fn drop(&mut self) {
        unsafe {
            let href = &mut *self.heap.get();
            href.safepoint().end();
            self.mutator.state_set(self.old_state, ThreadState::Waiting);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{atomic::AtomicU32, Arc};

    use crate::{
        safepoint::SafepointScope,
        semispace::{self},
    };

    const ITERATIONS: usize = 10000;

    #[test]
    fn stop_running_threads() {
        const THREADS: usize = 10;
        const RUNS: usize = 5;
        const SAFEPOINTS: usize = 3;
        let mut safepoint_count = 0;
        let mutator = semispace::instantiate_semispace(128 * 1024);
        for _ in 0..RUNS {
            let counter = Arc::new(AtomicU32::new(0));
            let mut handles = Vec::new();

            for _ in 0..THREADS {
                let c = counter.clone();
                handles.push(mutator.spawn_mutator(|mutator| {
                    let counter = c;

                    for i in 0..ITERATIONS {
                        counter.fetch_add(1, atomic::Ordering::AcqRel);
                        if i % 100 == 0 {
                            mutator.safepoint();
                        }
                    }
                }));
            }
            for _ in 0..SAFEPOINTS {
                let scope = SafepointScope::new(mutator.clone());
                safepoint_count += 1;
                drop(scope);
            }

            for handle in handles {
                handle.join(&mutator);
            }
            eprintln!("{}", counter.load(atomic::Ordering::Relaxed));
        }

        drop(mutator);
        assert_eq!(safepoint_count, RUNS * SAFEPOINTS);
    }
}
