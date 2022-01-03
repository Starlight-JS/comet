use crate::bitmap::SpaceBitmap;

pub struct ShenandoahFreeSet {
    mutator_free_bitmap: SpaceBitmap<4096>,
    collector_free_bitmap: SpaceBitmap<4096>,

    max: usize,
    mutator_leftmost: usize,
    mutator_rightmost: usize,
    collector_leftmost: usize,
    collector_rightmost: usize,
    capacity: usize,
    used: usize,
}

impl ShenandoahFreeSet {}
