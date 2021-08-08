use crate::allocator::normal::NormalAllocator;
use crate::allocator::overflow::OverflowAllocator;
use crate::allocator::Allocator;
use crate::block::SweepResult;
use crate::globals::LINE_SIZE;
use crate::heap::Heap;
use crate::internal::block_list::BlockList;
use crate::large_space::PreciseAllocation;
use crate::Config;
use crate::{
    block::Block, block_allocator::BlockAllocator, internal::space_bitmap::SpaceBitmap,
    large_space::LargeObjectSpace,
};

use std::ptr::null_mut;

pub const fn round_up(x: usize, y: usize) -> usize {
    ((x) + (y - 1)) & !(y - 1)
}

pub struct GlobalAllocator {
    pub(crate) block_allocator: Box<BlockAllocator>,
    pub(crate) large_space: LargeObjectSpace,
    pub(crate) live_bitmap: SpaceBitmap<8>,
    pub(crate) mark_bitmap: SpaceBitmap<8>,
    pub(crate) line_bitmap: SpaceBitmap<LINE_SIZE>,
    pub(crate) normal_allocator: NormalAllocator,
    pub(crate) overflow_allocator: OverflowAllocator,
}

impl GlobalAllocator {
    pub fn new(config: &Config) -> Self {
        let block_allocator = Box::new(BlockAllocator::new(config.heap_size));

        let mut global = Self {
            live_bitmap: SpaceBitmap::create(
                "live-bitmap",
                block_allocator.mmap.aligned(),
                block_allocator.size(),
            ),
            mark_bitmap: SpaceBitmap::create(
                "mark-bitmap",
                block_allocator.mmap.aligned(),
                block_allocator.size(),
            ),
            line_bitmap: SpaceBitmap::create(
                "line-bitmap",
                block_allocator.mmap.aligned(),
                block_allocator.size(),
            ),
            block_allocator,
            normal_allocator: NormalAllocator::new(null_mut(), null_mut()),
            overflow_allocator: OverflowAllocator::new(null_mut(), null_mut()),
            large_space: LargeObjectSpace::new(),
        };
        global.normal_allocator.block_allocator = &mut *global.block_allocator;
        global.overflow_allocator.block_allocator = &mut *global.block_allocator;
        global
    }
    pub fn large_allocation(&mut self, size: usize) -> (*mut u8, usize) {
        let cell = self.large_space.allocate(size);

        (cell.cast(), unsafe {
            (*PreciseAllocation::from_cell(cell)).cell_size()
        })
    }

    pub fn prepare_for_marking(&mut self, eden: bool) {
        self.large_space.prepare_for_marking(eden);
    }
    pub fn begin_marking(&mut self) -> BlockList {
        let mut blocks = BlockList::new();
        self.normal_allocator.get_all_blocks(&mut blocks);
        self.overflow_allocator.get_all_blocks(&mut blocks);
        blocks.for_each(|block| unsafe {
            self.mark_bitmap
                .clear_range(block as _, (*block).end() as _);
            self.line_bitmap.clear_range(block as _, (*block).end());
        });
        for alloc in self.large_space.allocations.iter() {
            unsafe {
                (**alloc).flip();
            }
        }
        blocks
    }

    pub(crate) fn release_memory(&mut self) {}

    pub(crate) fn sweep(&mut self, mut block_list: BlockList) {
        self.large_space.sweep();
        unsafe {
            while !block_list.is_empty() {
                let block = block_list.pop();

                match (*block).sweep::<true>(
                    &self.mark_bitmap,
                    &self.live_bitmap,
                    &self.line_bitmap,
                ) {
                    SweepResult::Empty => {
                        self.block_allocator.return_block(block);
                    }
                    SweepResult::Reuse => {
                        self.normal_allocator.recyclable_blocks.push(block);
                    }
                }
            }
        }
    }
    pub fn acquire_block(&self, heap: *const Heap) -> *mut Block {
        let mut block = null_mut::<Block>();
        if block.is_null() {
            block = self.block_allocator.get_block();
            unsafe {
                (*block).init(heap as _);
            }
        }

        block
    }
}
