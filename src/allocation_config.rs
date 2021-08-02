use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::Config;
pub struct AllocationConfig {
    pub threshold: u32,
    pub large_threshold: usize,
    pub block_allocations: AtomicU32,
    pub large_allocations: AtomicUsize,
}

impl AllocationConfig {
    pub fn new(threshold: u32, large_threshold: usize) -> Self {
        Self {
            threshold,
            large_threshold,
            block_allocations: AtomicU32::new(0),
            large_allocations: AtomicUsize::new(0),
        }
    }

    /// Returns true if the allocation threshold should be increased.
    ///
    /// The `blocks` argument should specify the current number of live blocks.
    pub fn should_increase_threshold(&self, blocks: usize, growth_threshold: f64) -> bool {
        let percentage = blocks as f64 / f64::from(self.threshold);

        percentage >= growth_threshold
    }

    pub fn should_increase_large_threshold(&self, alive: usize, growth_threshold: f64) -> bool {
        let percentage = alive as f64 / self.large_threshold as f64;
        percentage >= growth_threshold
    }

    pub fn increment_large_threshold(&mut self, growth_factor: f64) {
        self.large_threshold = (self.large_threshold as f64 * growth_factor).ceil() as usize;
    }

    pub fn increment_large_allocations(&self, size: usize) {
        self.large_allocations.fetch_add(size, Ordering::AcqRel);
    }

    pub fn increment_threshold(&mut self, growth_factor: f64) {
        self.threshold = (f64::from(self.threshold) * growth_factor).ceil() as u32;
    }

    pub fn update_after_collection(
        &mut self,
        config: &Config,
        blocks: usize,
        alive: usize,
    ) -> bool {
        let max = config.heap_growth_threshold;
        let factor = config.heap_growth_factor;
        let lmax = config.large_heap_growth_threshold;
        let lfactor = config.large_heap_growth_factor;

        self.block_allocations.store(0, Ordering::Relaxed);
        self.large_allocations.store(0, Ordering::Relaxed);
        let _ = if self.should_increase_threshold(blocks, max) {
            self.increment_threshold(factor);
            true
        } else {
            false
        };
        if self.should_increase_large_threshold(alive, lmax) {
            self.increment_large_threshold(lfactor);
        }
        false
    }

    pub fn allocation_threshold_exceeded(&self) -> bool {
        self.block_allocations.load(Ordering::Relaxed) >= self.threshold
            || self.large_allocations.load(Ordering::Relaxed) >= self.large_threshold
    }

    pub fn increment_allocations(&self) {
        self.block_allocations.fetch_add(1, Ordering::AcqRel);
    }
}
