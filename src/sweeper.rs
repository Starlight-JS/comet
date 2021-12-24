use crate::bitmap::SpaceBitmap;
use crate::rosalloc_space::RosAllocSpace;
use crate::utils::formatted_size;
use crossbeam_deque::{Injector, Steal, Stealer, Worker};
use rand::distributions::{Distribution, Uniform};
use rand::thread_rng;
use scoped_threadpool::Pool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

/// Sweep 256KB heap regions
const SWEEP_SIZE: usize = 128 * 1024 * 1024;

pub fn rosalloc_parallel_sweep(pool: &mut Pool, rosalloc: *mut RosAllocSpace) -> usize {
    let n_threads = pool.thread_count() as usize;
    let n_freed = AtomicUsize::new(0);
    let mut workers = Vec::with_capacity(n_threads);
    let mut stealers = Vec::with_capacity(n_threads);
    let injector = Injector::new();
    for _ in 0..n_threads {
        let w = Worker::new_lifo();
        let s = w.stealer();
        workers.push(w);
        stealers.push(s);
    }

    unsafe {
        let begin = (*rosalloc).begin();
        let end = (*rosalloc).end();
        let mut scan = begin;
        let mut n_chunks = 0;
        while scan < end {
            n_chunks += 1;
            let chunk_start = scan;
            let chunk_end = scan.add(SWEEP_SIZE).min(end);
            scan = chunk_end;

            injector.push((chunk_start as usize, chunk_end as usize));
        }
        /*println!(
            "Sweep {:p}->{:p} ({})",
            begin,
            end,
            formatted_size(end as usize - begin as usize)
        );*/

        debug_assert_eq!(scan, end);
    }
    let terminator = Terminator::new(n_threads);
    pool.scoped(|scoped| {
        let rosalloc = rosalloc as *mut _ as usize;
        for (task_id, worker) in workers.into_iter().enumerate() {
            let injector = &injector;
            let stealers = &stealers;
            let terminator = &terminator;
            let total_freed = &n_freed;
            scoped.execute(move || unsafe {
                let mut sweeper = Sweeper {
                    rosalloc_pointer: rosalloc,
                    injector,
                    stealers,
                    terminator,
                    total_freed,
                    task_id,
                    worker,
                };
                sweeper.run();
            })
        }
    });

    n_freed.load(Ordering::Relaxed)
}

struct Sweeper<'a> {
    task_id: usize,

    worker: Worker<(usize, usize)>,
    injector: &'a Injector<(usize, usize)>,
    stealers: &'a [Stealer<(usize, usize)>],
    terminator: &'a Terminator,
    rosalloc_pointer: usize,
    total_freed: &'a AtomicUsize,
}

pub struct Terminator {
    const_nworkers: usize,
    nworkers: AtomicUsize,
}

impl Terminator {
    pub fn new(number_workers: usize) -> Terminator {
        Terminator {
            const_nworkers: number_workers,
            nworkers: AtomicUsize::new(number_workers),
        }
    }

    pub fn try_terminate(&self) -> bool {
        if self.const_nworkers == 1 {
            return true;
        }

        if self.decrease_workers() {
            // reached 0, no need to wait
            return true;
        }

        thread::sleep(Duration::from_micros(1));
        self.zero_or_increase_workers()
    }

    fn decrease_workers(&self) -> bool {
        self.nworkers.fetch_sub(1, Ordering::Relaxed) == 1
    }

    fn zero_or_increase_workers(&self) -> bool {
        let mut nworkers = self.nworkers.load(Ordering::Relaxed);

        loop {
            if nworkers == 0 {
                return true;
            }

            let result = self.nworkers.compare_exchange(
                nworkers,
                nworkers + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );

            match result {
                Ok(_) => {
                    // Value was successfully increased again, workers didn't terminate in
                    // time. There is still work left.
                    return false;
                }

                Err(prev_nworkers) => {
                    nworkers = prev_nworkers;
                }
            }
        }
    }
}

impl<'a> Sweeper<'a> {
    fn pop(&mut self) -> Option<(usize, usize)> {
        self.pop_worker()
            .or_else(|| self.pop_global())
            .or_else(|| self.steal())
    }

    fn pop_worker(&mut self) -> Option<(usize, usize)> {
        self.worker.pop()
    }

    fn pop_global(&mut self) -> Option<(usize, usize)> {
        loop {
            let result = self.injector.steal_batch_and_pop(&mut self.worker);

            match result {
                Steal::Empty => break,
                Steal::Success(value) => return Some(value),
                Steal::Retry => continue,
            }
        }

        None
    }

    fn steal(&self) -> Option<(usize, usize)> {
        if self.stealers.len() == 1 {
            return None;
        }

        let mut rng = thread_rng();
        let range = Uniform::new(0, self.stealers.len());

        for _ in 0..2 * self.stealers.len() {
            let mut stealer_id = self.task_id;

            while stealer_id == self.task_id {
                stealer_id = range.sample(&mut rng);
            }

            let stealer = &self.stealers[stealer_id];

            loop {
                match stealer.steal_batch_and_pop(&self.worker) {
                    Steal::Empty => break,
                    Steal::Success(address) => return Some(address),
                    Steal::Retry => continue,
                }
            }
        }

        None
    }

    unsafe fn run(&mut self) {
        loop {
            let (sweep_begin, sweep_end) = if let Some((sweep_begin, sweep_end)) = self.pop() {
                (sweep_begin, sweep_end)
            } else if self.terminator.try_terminate() {
                break;
            } else {
                continue;
            };

            self.sweep_range(sweep_begin, sweep_end);
        }
    }

    unsafe fn sweep_range(&mut self, sweep_begin: usize, sweep_end: usize) {
        let space = &mut *(self.rosalloc_pointer as *mut RosAllocSpace);
        let live_bitmap = &*space.get_live_bitmap();
        let mark_bitmap = &*space.get_mark_bitmap();
        SpaceBitmap::<8>::sweep_walk(
            live_bitmap,
            mark_bitmap,
            sweep_begin,
            sweep_end,
            |ptrc, ptrs| {
                let pointers = std::slice::from_raw_parts(ptrs.cast::<*mut u8>(), ptrc);
                self.total_freed
                    .fetch_add((*space.rosalloc()).bulk_free(pointers), Ordering::Release);
            },
        );
    }
}
