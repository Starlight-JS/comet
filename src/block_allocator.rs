use std::{ptr::null_mut, sync::atomic::Ordering};

use crate::{
    block::Block, globals::IMMIX_BLOCK_SIZE, internal::block_list::AtomicBlockList, mmap::Mmap,
};

pub struct BlockAllocator {
    free_blocks: AtomicBlockList,

    //pub bitmap: SpaceBitmap<16>,
    pub data_bound: *mut u8,
    pub data: *mut u8,
    pub mmap: Mmap,
}

impl BlockAllocator {
    pub fn total_blocks(&self) -> usize {
        (self.mmap.end() as usize - self.mmap.aligned() as usize) / IMMIX_BLOCK_SIZE
    }
    pub fn start(&self) -> *mut u8 {
        self.mmap.aligned()
    }

    pub fn end(&self) -> *mut u8 {
        self.mmap.end()
    }
    pub fn size(&self) -> usize {
        self.end() as usize - self.start() as usize
    }
    pub fn new(size: usize) -> BlockAllocator {
        let map = Mmap::new(size);

        let this = Self {
            data: map.aligned(),
            data_bound: map.end(),
            free_blocks: AtomicBlockList::new(),

            mmap: map,
        };
        debug_assert!(this.data as usize % IMMIX_BLOCK_SIZE == 0);
        this.mmap.commit(this.mmap.start(), IMMIX_BLOCK_SIZE);
        this
    }

    /// Get a new block aligned to `BLOCK_SIZE`.
    pub fn get_block(&self) -> *mut Block {
        match self.free_blocks.take_free() {
            x if x.is_null() => match self.build_block() {
                Some(x) => x,
                None => null_mut(),
            },
            x => {
                self.mmap.commit(x as _, IMMIX_BLOCK_SIZE);
                Block::new(x as _);
                x
            }
        }
    }

    pub fn is_in_space(&self, object: *const u8) -> bool {
        self.mmap.start() < object as *mut u8 && object <= self.data_bound
    }
    #[allow(unused_unsafe)]
    fn build_block(&self) -> Option<*mut Block> {
        unsafe {
            let data = as_atomic!(&self.data;AtomicUsize);
            let mut old = data.load(Ordering::Relaxed);
            let mut new;
            loop {
                new = old + IMMIX_BLOCK_SIZE;
                if new > self.data_bound as usize {
                    return None;
                }
                let res = data.compare_exchange_weak(old, new, Ordering::SeqCst, Ordering::Relaxed);
                match res {
                    Ok(_) => break,
                    Err(x) => old = x,
                }
            }
            debug_assert!(
                old % IMMIX_BLOCK_SIZE == 0,
                "block is not aligned for block_size"
            );
            self.mmap.commit(old as *mut u8, IMMIX_BLOCK_SIZE);
            Some(old as *mut Block)
        }
    }

    /// Return a collection of blocks.
    pub fn return_blocks(&mut self, blocks: impl Iterator<Item = *mut Block>) {
        blocks.for_each(|block| unsafe {
            (*block).allocated = 0;
            self.mmap.dontneed(block as *mut u8, IMMIX_BLOCK_SIZE); // MADV_DONTNEED or MEM_DECOMMIT
            self.free_blocks.add_free(block);
        });
    }
    pub fn return_block(&mut self, block: *mut Block) {
        unsafe {
            (*block).allocated = 0;
        }
        //self.mmap.dontneed(block as *mut u8, IMMIX_BLOCK_SIZE); // MADV_DONTNEED or MEM_DECOMMIT
        unsafe {
            self.free_blocks.add_free(block);
        }
    }

    /// Return the number of unallocated blocks.
    pub fn available_blocks(&self) -> usize {
        let nblocks = ((self.data_bound as usize) - (self.data as usize)) / IMMIX_BLOCK_SIZE;

        nblocks + self.free_blocks.count()
    }
}
