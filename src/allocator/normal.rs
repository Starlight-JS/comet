use crate::{
    block::Block,
    block_allocator::BlockAllocator,
    globals::{IMMIX_BLOCK_SIZE, LINE_SIZE},
    internal::{block_list::BlockList, space_bitmap::SpaceBitmap},
};

use super::{Allocator, BlockTuple};

/// The `NormalAllocator` is the standard allocator to allocate objects within
/// the immix space.
///
/// Objects smaller than `MEDIUM_OBJECT` bytes are
pub struct NormalAllocator {
    /// The global `BlockAllocator` to get new blocks from.
    pub(crate) block_allocator: *mut BlockAllocator,

    /// The exhausted blocks.
    unavailable_blocks: BlockList,

    /// The blocks with holes to recycle before requesting new blocks..
    pub(crate) recyclable_blocks: BlockList,

    /// The current block to allocate from.
    current_block: Option<BlockTuple>,
    pub(crate) line_bitmap: *const SpaceBitmap<LINE_SIZE>,
}
impl NormalAllocator {
    /// Create a new `NormalAllocator` backed by the given `BlockAllocator`.
    pub fn new(
        block_allocator: *mut BlockAllocator,
        bitmap: *const SpaceBitmap<LINE_SIZE>,
    ) -> NormalAllocator {
        NormalAllocator {
            block_allocator: block_allocator,
            unavailable_blocks: BlockList::new(),
            recyclable_blocks: BlockList::new(),
            current_block: None,
            line_bitmap: bitmap,
        }
    }

    /// Set the recyclable blocks.
    pub fn set_recyclable_blocks(&mut self, blocks: BlockList) {
        self.recyclable_blocks = blocks;
    }
}

impl Allocator for NormalAllocator {
    fn get_all_blocks(&mut self, list: &mut BlockList) {
        while !self.unavailable_blocks.is_empty() {
            list.push(self.unavailable_blocks.pop());
        }
        while !self.recyclable_blocks.is_empty() {
            list.push(self.recyclable_blocks.pop());
        }
        if let Some(block) = self.current_block.take() {
            list.push((block).0);
        }
    }

    fn take_current_block(&mut self) -> Option<BlockTuple> {
        self.current_block.take()
    }

    fn put_current_block(&mut self, block_tuple: BlockTuple) {
        self.current_block = Some(block_tuple);
    }

    fn get_new_block(&mut self) -> Option<BlockTuple> {
        unsafe {
            let block = (*self.block_allocator).get_block();
            if block.is_null() {
                return None;
            }
            (*block).set_allocated();
            Some((block, LINE_SIZE as u16, (IMMIX_BLOCK_SIZE - 1) as u16))
        }
    }
    fn line_bitmap(&self) -> &SpaceBitmap<LINE_SIZE> {
        unsafe { &*self.line_bitmap }
    }
    fn handle_no_hole(&mut self, size: usize) -> Option<BlockTuple> {
        if size >= LINE_SIZE {
            None
        } else {
            match self.recyclable_blocks.pop() {
                x if x.is_null() => None,
                block => {
                    match unsafe { (*block).scan_block(&*self.line_bitmap, (LINE_SIZE - 1) as u16) }
                    {
                        None => {
                            self.handle_full_block(block);
                            self.handle_no_hole(size)
                        }
                        Some((low, high)) => self
                            .scan_for_hole(size, (block, low, high))
                            .or_else(|| self.handle_no_hole(size)),
                    }
                }
            }
        }
    }

    fn handle_full_block(&mut self, block: *mut Block) {
        self.unavailable_blocks.push(block);
    }
}
