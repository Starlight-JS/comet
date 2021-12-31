use std::{mem::size_of, sync::atomic::AtomicPtr};

use atomic::Atomic;

use crate::{
    shenandoah::region,
    utils::{align_down, align_up, align_usize, formatted_size},
};

/*
 Region state is described by a state machine. Transitions are guarded by
 heap lock, which allows changing the state of several regions atomically.
 Region states can be logically aggregated in groups.
   "Empty":
   .................................................................
   .                                                               .
   .                                                               .
   .         Uncommitted  <-------  Committed <------------------------\
   .              |                     |                          .   |
   .              \---------v-----------/                          .   |
   .                        |                                      .   |
   .........................|.......................................   |
                            |                                          |
   "Active":                |                                          |
   .........................|.......................................   |
   .                        |                                      .   |
   .      /-----------------^-------------------\                  .   |
   .      |                                     |                  .   |
   .      v                                     v    "Humongous":  .   |
   .   Regular ---\-----\     ..................O................  .   |
   .     |  ^     |     |     .                 |               .  .   |
   .     |  |     |     |     .                 *---------\     .  .   |
   .     v  |     |     |     .                 v         v     .  .   |
   .    Pinned  Cset    |     .  HStart <--> H/Start   H/Cont   .  .   |
   .       ^    / |     |     .  Pinned         v         |     .  .   |
   .       |   /  |     |     .                 *<--------/     .  .   |
   .       |  v   |     |     .                 |               .  .   |
   .  CsetPinned  |     |     ..................O................  .   |
   .              |     |                       |                  .   |
   .              \-----\---v-------------------/                  .   |
   .                        |                                      .   |
   .........................|.......................................   |
                            |                                          |
   "Trash":                 |                                          |
   .........................|.......................................   |
   .                        |                                      .   |
   .                        v                                      .   |
   .                      Trash ---------------------------------------/
   .                                                               .
   .                                                               .
   .................................................................
 Transition from "Empty" to "Active" is first allocation. It can go from {Uncommitted, Committed}
 to {Regular, "Humongous"}. The allocation may happen in Regular regions too, but not in Humongous.
 Transition from "Active" to "Trash" is reclamation. It can go from CSet during the normal cycle,
 and from {Regular, "Humongous"} for immediate reclamation. The existence of Trash state allows
 quick reclamation without actual cleaning up.
 Transition from "Trash" to "Empty" is recycling. It cleans up the regions and corresponding metadata.
 Can be done asynchronously and in bulk.
 Note how internal transitions disallow logic bugs:
   a) No region can go Empty, unless properly reclaimed/recycled;
   b) No region can go Uncommitted, unless reclaimed/recycled first;
   c) Only Regular regions can go to CSet;
   d) Pinned cannot go Trash, thus it could never be reclaimed until unpinned;
   e) Pinned cannot go CSet, thus it never moves;
   f) Humongous cannot be used for regular allocations;
   g) Humongous cannot go CSet, thus it never moves;
   h) Humongous start can go pinned, and thus can be protected from moves (humongous continuations should
      follow associated humongous starts, not pinnable/movable by themselves);
   i) Empty cannot go Trash, avoiding useless work;
   j) ...
*/
pub struct ShenandoahHeapRegion {
    index: usize,
    bottom: *mut u8,
    end: *mut u8,

    new_top: *mut u8,
    empty_time: f64,

    state: RegionState,

    top: *mut u8,

    tlab_allocs: usize,
    gclab_allocs: usize,

    live_data: Atomic<usize>,
    update_watermark: AtomicPtr<u8>,
}

#[derive(Default, Clone, Copy)]
pub struct ShenandoahOptions {
    pub region_size_bytes: usize,
    pub region_size_words: usize,
    pub region_size_bytes_shift: usize,
    pub region_size_words_shift: usize,
    pub region_size_bytes_mask: usize,
    pub region_size_words_mask: usize,
    pub region_count: usize,
    pub humongous_threshold_words: usize,
    pub humongous_threshold_bytes: usize,
    pub max_tlab_size_words: usize,
    pub max_tlab_size_bytes: usize,
    pub max_heap_size: usize,
}
impl std::fmt::Debug for ShenandoahOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "ShenandoahOptions:")?;
        writeln!(
            f,
            "\tregion_size_bytes: {}",
            formatted_size(self.region_size_bytes)
        )?;
        writeln!(f, "\tregion_size_words: {}", self.region_size_words)?;
        writeln!(
            f,
            "\tregion_size_bytes_shift: {}",
            self.region_size_bytes_shift
        )?;
        writeln!(
            f,
            "\tregion_size_words_shift: {}",
            self.region_size_words_shift
        )?;
        writeln!(
            f,
            "\tregion_size_words_mask: {}",
            self.region_size_words_mask
        )?;
        writeln!(
            f,
            "\tregion_size_bytes_mask: {}",
            self.region_size_bytes_mask
        )?;
        writeln!(f, "\tregion_count: {}", self.region_count)?;
        writeln!(
            f,
            "\thumongous_threshold_bytes: {}",
            formatted_size(self.humongous_threshold_bytes)
        )?;
        writeln!(
            f,
            "\thumongous_threshold_words: {}",
            self.humongous_threshold_words
        )?;
        writeln!(
            f,
            "\tmax_tlab_size_bytes: {}",
            formatted_size(self.max_tlab_size_bytes)
        )?;
        writeln!(f, "\tmax_heap_size: {}", formatted_size(self.max_heap_size))
    }
}
impl ShenandoahHeapRegion {
    pub const MIN_REGION_SIZE: usize = 256 * 1024;
    pub const MIN_NUM_REGIONS: usize = 10;
    pub const MAX_REGION_SIZE: usize = 32 * 1024 * 1024;
    pub fn setup_sizes(
        mut max_heap_size: usize,
        min_region_size: Option<usize>,
        target_num_regions: Option<usize>,
        max_region_size: Option<usize>,
    ) -> ShenandoahOptions {
        let mut opts = ShenandoahOptions::default();
        let mut region_size;
        let min_region_size = min_region_size
            .map(|x| {
                if x < Self::MIN_REGION_SIZE {
                    Self::MIN_REGION_SIZE
                } else {
                    x
                }
            })
            .unwrap_or_else(|| Self::MIN_REGION_SIZE);
        let target_num_regions = target_num_regions.unwrap_or_else(|| 2048);
        let max_region_size = max_region_size.unwrap_or_else(|| Self::MAX_REGION_SIZE);
        if min_region_size > max_heap_size / Self::MIN_NUM_REGIONS {
            panic!("Max heap size ({}) is too low to afford the minimum number of regions ({}) of minimum region size ({})",
                formatted_size(max_heap_size),Self::MIN_NUM_REGIONS,formatted_size(min_region_size)
            );
        }

        region_size = max_heap_size / target_num_regions;
        region_size = region_size.max(min_region_size);
        region_size = max_region_size.min(region_size);

        let page_size = 4096; // todo: use OS functions to determine this

        region_size = align_usize(region_size, page_size);

        max_heap_size = align_up(max_heap_size, page_size);

        let region_size_log = (region_size as f64).log2() as usize;
        region_size = 1 << region_size_log;
        opts.region_size_bytes_shift = region_size_log;
        opts.region_size_bytes = region_size;
        opts.region_size_words = region_size >> 3;
        opts.region_size_words_mask = opts.region_size_words - 1;
        opts.region_size_bytes_mask = opts.region_size_bytes - 1;
        opts.region_size_words_shift = opts.region_size_bytes_shift - 3;
        opts.region_count =
            align_up(max_heap_size, opts.region_size_bytes) / opts.region_size_bytes;
        opts.humongous_threshold_words = opts.region_size_words * 100 / 100;
        opts.humongous_threshold_words = align_down(opts.humongous_threshold_words, 8);
        opts.humongous_threshold_bytes = opts.humongous_threshold_words * size_of::<usize>();
        opts.max_tlab_size_words = align_down(
            (opts.region_size_words / 8).min(opts.humongous_threshold_words),
            8,
        );
        opts.max_tlab_size_bytes = opts.max_tlab_size_words * size_of::<usize>();
        opts.max_heap_size = max_heap_size;
        opts
    }
}

/// Shenandoah in OpenJDK actually supports pinning but we do not support it there.

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum RegionState {
    EmptyUncommitted,
    EmptyCommitted,
    Regular,
    HumongousStart,
    HumongousCont,
    CSet,
    Trash,
}
