use crate::allocation_config::AllocationConfig;
use crate::block::SweepResult;
use crate::large_space::PreciseAllocation;
use crate::Config;
use crate::{
    block::Block,
    block_allocator::BlockAllocator,
    internal::{block_list::AtomicBlockList, space_bitmap::SpaceBitmap, BLOCK_SIZE},
    large_space::LargeObjectSpace,
};
use std::mem::size_of;
use std::ptr::null_mut;

/// Sizes up to this amount get a size class for each size step.
pub const PRECISE_CUTOFF: usize = 80;
const SIZE_STEP: usize = 16;
pub const fn round_up(x: usize, y: usize) -> usize {
    ((x) + (y - 1)) & !(y - 1)
}
const LARGE_CUTOFF: usize = ((BLOCK_SIZE - size_of::<Block>()) / 2) & !(SIZE_STEP - 1);
const BLOCK_PAYLOAD: usize = BLOCK_SIZE - size_of::<Block>();
fn generate_size_classes(dump_size_classes: bool, sz_class_progression: f64) -> Vec<usize> {
    let mut result = vec![];
    let add = |result: &mut Vec<usize>, size_class| {
        logln_if!(dump_size_classes, "Adding size class: {}", size_class);
        if result.is_empty() {
            assert_eq!(size_class, 16);
        }
        result.push(size_class);
    };

    let mut size = 16;
    while size < PRECISE_CUTOFF {
        add(&mut result, size);
        size += SIZE_STEP;
    }
    logln_if!(
        dump_size_classes,
        "       Block payload size: {}",
        BLOCK_SIZE - offsetof!(Block.data_start)
    );

    for i in 0.. {
        let approximate_size = PRECISE_CUTOFF as f64 * sz_class_progression.powi(i);
        logln_if!(
            dump_size_classes,
            "     Next size class as a double: {}",
            approximate_size
        );
        let approximate_size_in_bytes = approximate_size as usize;
        logln_if!(
            dump_size_classes,
            "     Next size class as bytes: {}",
            approximate_size_in_bytes
        );
        assert!(approximate_size_in_bytes >= PRECISE_CUTOFF);

        if approximate_size_in_bytes >= LARGE_CUTOFF {
            break;
        }
        let size_class = round_up(approximate_size_in_bytes, SIZE_STEP);
        logln_if!(dump_size_classes, "     Size class: {}", size_class);

        let cells_per_block = BLOCK_PAYLOAD / size_class;
        let possibly_better_size_class = (BLOCK_PAYLOAD / cells_per_block) & !(SIZE_STEP - 1);
        logln_if!(
            dump_size_classes,
            "     Possibly better size class: {}",
            possibly_better_size_class
        );
        let original_wastage = BLOCK_PAYLOAD - cells_per_block * size_class;
        let new_wastage = (possibly_better_size_class - size_class) * cells_per_block;
        logln_if!(
            dump_size_classes,
            "    Original wastage: {}, new wastage: {}",
            original_wastage,
            new_wastage
        );

        let better_size_class = if new_wastage > original_wastage {
            size_class
        } else {
            possibly_better_size_class
        };
        logln_if!(
            dump_size_classes,
            "    Choosing size class: {}",
            better_size_class
        );
        if Some(better_size_class) == result.last().copied() {
            // when size class step is too small
            continue;
        }

        if better_size_class > LARGE_CUTOFF {
            break;
        }
        add(&mut result, better_size_class);
    }
    // Manually inject size classes for objects we know will be allocated in high volume.

    add(&mut result, 256);
    //add(&mut result, size_of::<JsObject>());
    result.sort_unstable();
    result.dedup();
    result.shrink_to_fit();
    logln_if!(dump_size_classes, "Heap size class dump: {:?}", result);

    result
}

pub const NUM_SIZE_CLASSES: usize = LARGE_CUTOFF / SIZE_STEP + 1;
fn build_size_class_table(
    dump: bool,
    progression: f64,
    table: &mut [usize],
    cons: impl Fn(usize) -> usize,
    default_cons: impl Fn(usize) -> usize,
) {
    let mut next_index = 0;
    for sz in generate_size_classes(dump, progression) {
        let entry = cons(sz);
        let index = size_class_to_index(sz);
        for i in next_index..=index {
            table[i] = entry;
        }
        next_index = index + 1;
    }
    for i in next_index..NUM_SIZE_CLASSES {
        table[i] = default_cons(index_to_size_class(i));
    }
}
fn initialize_size_class_for_step_size(dump: bool, progression: f64, table: &mut [usize]) {
    build_size_class_table(dump, progression, table, |sz| sz, |sz| sz);
}

pub const fn size_class_to_index(size: usize) -> usize {
    (size + SIZE_STEP - 1) / SIZE_STEP
}

pub fn index_to_size_class(index: usize) -> usize {
    let result = index * SIZE_STEP;
    debug_assert_eq!(size_class_to_index(result), index);
    result
}

pub struct GlobalAllocator {
    pub(crate) free_blocks: Box<[AtomicBlockList]>,
    pub(crate) unavail_blocks: Box<[AtomicBlockList]>,
    pub(crate) block_allocator: BlockAllocator,
    large_space: LargeObjectSpace,
    pub(crate) live_bitmap: SpaceBitmap<16>,
    config: AllocationConfig,
    pub(crate) size_class_for_size_step: Box<[usize]>,
}

impl GlobalAllocator {
    pub fn new(config: &Config) -> Self {
        let mut table = vec![0; NUM_SIZE_CLASSES];
        initialize_size_class_for_step_size(
            config.dump_size_classes,
            config.size_class_progression,
            &mut table,
        );
        let block_allocator = BlockAllocator::new(config.heap_size);

        Self {
            free_blocks: vec![AtomicBlockList::new(); NUM_SIZE_CLASSES].into_boxed_slice(),
            unavail_blocks: vec![AtomicBlockList::new(); NUM_SIZE_CLASSES].into_boxed_slice(),
            live_bitmap: SpaceBitmap::create(
                "live-bitmap",
                block_allocator.mmap.start(),
                config.heap_size,
            ),
            block_allocator,
            large_space: LargeObjectSpace::new(),
            config: AllocationConfig::new(config.block_threshold as _, config.large_threshold),
            size_class_for_size_step: table.into_boxed_slice(),
        }
    }
    pub fn large_allocation(&mut self, size: usize) -> *mut u8 {
        unsafe {
            if self.config.allocation_threshold_exceeded() {
                return null_mut();
            }
            let cell = self.large_space.allocate(size);
            let prec = PreciseAllocation::from_cell(cell);
            self.config.increment_large_allocations((*prec).cell_size());

            cell.cast()
        }
    }
    pub(crate) fn sweep(&mut self) {
        unsafe {
            for (index, class) in self.free_blocks.iter_mut().enumerate() {
                let new_list = AtomicBlockList::new();
                loop {
                    let block = class.take_free();
                    if block.is_null() {
                        break;
                    }
                    match (*block).sweep(&self.live_bitmap) {
                        SweepResult::Empty => self.block_allocator.return_block(block),
                        SweepResult::Full => self.unavail_blocks[index].add_free(block),
                        SweepResult::Reusable => new_list.add_free(block),
                    }
                }
                *class = new_list;
            }
            for (index, class) in self.unavail_blocks.iter_mut().enumerate() {
                let new_list = AtomicBlockList::new();
                loop {
                    let block = class.take_free();
                    if block.is_null() {
                        break;
                    }
                    match (*block).sweep(&self.live_bitmap) {
                        SweepResult::Empty => self.block_allocator.return_block(block),
                        SweepResult::Reusable => self.free_blocks[index].add_free(block),
                        SweepResult::Full => new_list.add_free(block),
                    }
                }
                *class = new_list;
            }
            self.large_space.sweep();
        }
    }
    pub fn acquire_block(&self, size_class: usize) -> *mut Block {
        let freelist = &self.free_blocks[size_class];
        if self.config.allocation_threshold_exceeded() {
            return null_mut();
        }
        let mut block = freelist.take_free();
        if block.is_null() {
            // todo: should increment_allocations also work on recyclable blocks (from self.free_blocks)?
            self.config.increment_allocations();
            block = self.block_allocator.get_block();
            unsafe {
                (*block).init(self.size_class_for_size_step[size_class] as _);
            }
        }

        block
    }
}
