use std::sync::atomic::Ordering;

use crate::{block::Block, internal::BLOCK_SIZE, mmap::Mmap};

pub struct BlockAllocator {
    #[cfg(feature = "threaded")]
    lock: ReentrantMutex,
    free_blocks: Vec<*mut Block>,

    //pub bitmap: SpaceBitmap<16>,
    pub data_bound: *mut u8,
    pub data: *mut u8,
    pub mmap: Mmap,
}

impl BlockAllocator {
    pub fn total_blocks(&self) -> usize {
        (self.mmap.end() as usize - self.mmap.aligned() as usize) / BLOCK_SIZE
    }
    pub fn new(size: usize) -> BlockAllocator {
        let map = Mmap::new(size);

        let this = Self {
            #[cfg(feature = "threaded")]
            lock: ReentrantMutex::new(),
            data: map.aligned(),
            data_bound: map.end(),
            free_blocks: Vec::new(),

            mmap: map,
        };
        debug_assert!(this.data as usize % BLOCK_SIZE == 0);
        this.mmap.commit(this.mmap.start(), BLOCK_SIZE);
        this
    }

    /// Get a new block aligned to `BLOCK_SIZE`.
    pub fn get_block(&mut self) -> Option<*mut Block> {
        if self.free_blocks.is_empty() {
            return self.build_block();
        }

        let block = self
            .free_blocks
            .pop()
            .map(|x| {
                self.mmap.commit(x as *mut u8, BLOCK_SIZE);
                Block::new(x as *mut u8);
                x
            })
            .or_else(|| self.build_block());
        if block.is_none() {
            panic!("OOM");
        }
        block
    }

    pub fn is_in_space(&self, object: *const u8) -> bool {
        self.mmap.start() < object as *mut u8 && object <= self.data_bound
    }
    #[allow(unused_unsafe)]
    fn build_block(&mut self) -> Option<*mut Block> {
        unsafe {
            let data = as_atomic!(&self.data;AtomicUsize);
            let mut old = data.load(Ordering::Relaxed);
            let mut new;
            loop {
                new = old + BLOCK_SIZE;
                if new > self.data_bound as usize {
                    return None;
                }
                let res = data.compare_exchange_weak(old, new, Ordering::SeqCst, Ordering::Relaxed);
                match res {
                    Ok(_) => break,
                    Err(x) => old = x,
                }
            }
            debug_assert!(old % BLOCK_SIZE == 0, "block is not aligned for block_size");
            self.mmap.commit(old as *mut u8, BLOCK_SIZE);
            Some(old as *mut Block)
        }
    }

    /// Return a collection of blocks.
    pub fn return_blocks(&mut self, blocks: impl Iterator<Item = *mut Block>) {
        blocks.for_each(|block| {
            self.mmap.dontneed(block as *mut u8, BLOCK_SIZE); // MADV_DONTNEED or MEM_DECOMMIT
            self.free_blocks.push(block);
        });
    }
    pub fn return_block(&mut self, block: *mut Block) {
        self.mmap.dontneed(block as *mut u8, BLOCK_SIZE); // MADV_DONTNEED or MEM_DECOMMIT
        self.free_blocks.push(block);
    }

    /// Return the number of unallocated blocks.
    pub fn available_blocks(&self) -> usize {
        let nblocks = ((self.data_bound as usize) - (self.data as usize)) / BLOCK_SIZE;

        nblocks + self.free_blocks.len()
    }
}
