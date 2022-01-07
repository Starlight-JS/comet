use crate::utils::mmap::Mmap;

pub struct ShenandoahHeap {
    num_regions: usize,
    heap_region_special: bool,
    heap_region: Mmap,
}
