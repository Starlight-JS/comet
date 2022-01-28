use crate::{bitmap::LineMarkTable, utils::align_down};

use super::{space::ImmixSpace, ImmixBlock, IMMIX_BLOCK_SIZE};

/// Represents chunk that contains [ImmixBlock]'s. Each chunk can store up to 128 blocks but
/// only 127 blocks are available for use because first 32KB of memory is reserved for chunk metadata.
///
/// Chunk metadata at the moment contains only line mark table for marking block lines, it takes 2KB of memory per each chunk.
pub struct Chunk {
    line_mark_bitmap: LineMarkTable,
}

pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;
/// Number of blocks in single chunk.
pub const CHUNK_BLOCKS: usize = CHUNK_SIZE / IMMIX_BLOCK_SIZE;

impl Chunk {
    pub fn new(at: *mut u8) -> *mut Chunk {
        unsafe {
            at.cast::<Self>().write(Self {
                line_mark_bitmap: LineMarkTable::create("line mark table", at, CHUNK_SIZE),
            });
            // Instantiate line bitmap per chunk so we clear marks only per chunk rather than entire heap.
            (*at.cast::<Self>()).line_mark_bitmap.init_bitmap();
            at.cast()
        }
    }
    pub fn start(&self) -> *mut u8 {
        self as *const Self as *mut u8
    }

    pub fn end(&self) -> *mut u8 {
        (self as *const Self as usize + CHUNK_SIZE) as _
    }

    pub fn blocks_start(&self) -> *mut ImmixBlock {
        self.start().cast()
    }
    pub fn block(&self, index: usize) -> *mut ImmixBlock {
        unsafe {
            self.blocks_start()
                .cast::<u8>()
                .add(index * IMMIX_BLOCK_SIZE)
                .cast()
        }
    }
    pub fn line_mark_table(&self) -> &LineMarkTable {
        &self.line_mark_bitmap
    }
    pub fn line_mark_table_mut(&mut self) -> &mut LineMarkTable {
        &mut self.line_mark_bitmap
    }

    /// Sweep single chunk. If chunk is empty it's entry in chunk map is cleared
    pub fn sweep(&mut self, space: &ImmixSpace, sweep_color: u8) {
        let mut cursor = 1;
        let mut allocated_blocks = 0;
        while cursor < CHUNK_BLOCKS {
            let block = self.block(cursor);
            unsafe {
                if !(*block).sweep(space, sweep_color) {
                    allocated_blocks += 1;
                }
            }
            cursor += 1;
        }
        if allocated_blocks == 0 {
            space.chunk_map.clear(self as *const Self as *const _);
        }
    }

    pub fn align(addr: *const u8) -> *mut u8 {
        align_down(addr as _, CHUNK_SIZE) as _
    }
}
