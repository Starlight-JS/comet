use crate::{
    block::Block,
    block_allocator::BlockAllocator,
    globals::{IMMIX_BLOCK_SIZE, LINE_SIZE},
    internal::{block_list::BlockList, space_bitmap::SpaceBitmap},
};

use super::{Allocator, BlockTuple};

/// The `OverflowAllocator` is used to allocate *medium* sized objects
/// (objects of at least `MEDIUM_OBJECT` bytes size) within the immix space to
/// limit fragmentation in the `NormalAllocator`.
pub struct OverflowAllocator {
    /// The global `BlockAllocator` to get new blocks from.
    pub(crate) block_allocator: *mut BlockAllocator,
    pub(crate) line_bitmap: *const SpaceBitmap<LINE_SIZE>,
    /// The exhausted blocks.
    unavailable_blocks: BlockList,

    /// The current block to allocate from.
    current_block: Option<BlockTuple>,
}

impl OverflowAllocator {
    /// Create a new `OverflowAllocator` backed by the given `BlockAllocator`.
    pub fn new(
        block_allocator: *mut BlockAllocator,
        bitmap: *const SpaceBitmap<LINE_SIZE>,
    ) -> OverflowAllocator {
        OverflowAllocator {
            block_allocator: block_allocator,
            unavailable_blocks: BlockList::new(),
            line_bitmap: unsafe { &*bitmap },
            current_block: None,
        }
    }
}

impl Allocator for OverflowAllocator {
    fn line_bitmap(&self) -> &SpaceBitmap<LINE_SIZE> {
        unsafe { &*self.line_bitmap }
    }
    fn get_all_blocks(&mut self, list: &mut BlockList) {
        while !self.unavailable_blocks.is_empty() {
            list.push(self.unavailable_blocks.pop());
        }
        self.current_block.take().iter().for_each(|block| {
            list.push(block.0);
        });
        /*self.unavailable_blocks
        .drain(..)
        .chain(self.current_block.take().map(|b| b.0))
        .collect()*/
    }

    fn take_current_block(&mut self) -> Option<BlockTuple> {
        self.current_block.take()
    }

    fn put_current_block(&mut self, block_tuple: BlockTuple) {
        self.current_block = Some(block_tuple);
    }

    fn get_new_block(&mut self) -> Option<BlockTuple> {
        /*self.block_allocator
        .borrow_mut()
        .get_block()
        .map(|b| unsafe {
            (*b).set_allocated();
            b
        })
        .map(|block| (block, LINE_SIZE as u16, (BLOCK_SIZE - 1) as u16))*/
        unsafe {
            let block = (*self.block_allocator).get_block();
            if block.is_null() {
                return None;
            }
            (*block).set_allocated();
            Some((block, LINE_SIZE as _, IMMIX_BLOCK_SIZE as u16 - 1))
        }
    }

    #[allow(unused_variables)]
    fn handle_no_hole(&mut self, size: usize) -> Option<BlockTuple> {
        None
    }

    fn handle_full_block(&mut self, block: *mut Block) {
        self.unavailable_blocks.push(block);
    }
}
