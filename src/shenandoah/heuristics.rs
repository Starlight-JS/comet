use super::{collection_set::ShenandoahCollectionSet, region::ShenandoahHeapRegion};

pub struct RegionData {
    pub region: *mut ShenandoahHeapRegion,
    pub garbage: usize,
}

pub trait ShenandoahHeuristics {
    /// recover from penalties
    const CONCURRENT_ADJUST: isize = -1;
    /// how much to penalize average GC duration history on Degenerated GC
    const DEGENERATE_PENALTY: isize = 10;
    /// how much to penalize average GC duration history on Full GC
    const FULL_PENALTY: isize = 20;

    fn region_data(&self) -> *mut RegionData;
    fn set_region_data(&mut self, data: *mut RegionData);

    fn degenerated_cycles_in_a_row(&self) -> u32;
    fn successful_cycles_in_a_row(&self) -> u32;
    fn cycle_start(&self) -> f64;
    fn last_cycle_end(&self) -> f64;

    fn gc_times_learned(&self) -> usize;
    fn gc_time_penalties(&self) -> isize;
    fn set_gc_time_penalties(&mut self, x: isize);

    fn choose_collection_set_from_regiondata(
        &mut self,
        set: &mut ShenandoahCollectionSet,
        data: *mut RegionData,
        data_size: usize,
        free: usize,
    );

    fn should_start_gc(&self, _guaranteed_interval: f64) -> bool {
        false
    }
}
