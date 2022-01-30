use super::*;
use crate::{
    bitmap::{ObjectStartBitmap, SpaceBitmap},
    utils::mmap::Mmap,
};
pub struct ImmixSpace {
    map: Mmap,
    pub free_blocks: BlockList,
    pub reusable_blocks: BlockList,
    pub n_chunks: usize,
    pub chunk_map: ChunkMap,
    pub target_footprint: AtomicUsize,
    pub num_bytes_allocated: AtomicUsize,
    pub initial_size: usize,
    pub min_heap_size: usize,
    pub max_heap_size: usize,
    pub growth_limit: usize,
    pub mark_bitmap: SpaceBitmap<8>,
}

impl ImmixSpace {
    pub fn target_footprint(&self) -> &AtomicUsize {
        &self.target_footprint
    }
    pub fn new(
        size: usize,
        mut initial_size: usize,
        min_heap_size: usize,
        max_heap_size: usize,
        verbose: bool,
    ) -> ImmixSpace {
        let size = round_up(size as _, CHUNK_SIZE as _);
        let mmap = Mmap::new(size as _, CHUNK_SIZE);
        let aligned_size = mmap.end() as usize - mmap.aligned_start() as usize;

        let n_chunks = aligned_size / CHUNK_SIZE;
        let free_list = BlockList::new();
        let mut n_blocks = 0;
        let chunk_map = ChunkMap::create("chunk-map", mmap.aligned_start(), aligned_size);
        for i in 0..n_chunks {
            unsafe {
                let chunk = Chunk::new(mmap.aligned_start().add(i * CHUNK_SIZE));

                for i in 1..CHUNK_BLOCKS {
                    let block = (*chunk).block(i);

                    assert!(
                        (block as usize) < (*chunk).end() as usize,
                        "Block out of bounds of chunk: {:p} < 0x{:x}",
                        block,
                        chunk as usize + CHUNK_SIZE
                    );

                    // mark block as unallocated and push it to free list.
                    (*block).set_state(BlockState::Unallocated);
                    free_list.push(block);
                    n_blocks += 1;
                }
            }
        }
        if verbose {
            eprintln!(
                "[immix] Instantiated Immix space {:p}->{:p}({}), chunks: {}, blocks: {}",
                mmap.start(),
                mmap.end(),
                formatted_size(aligned_size),
                n_chunks,
                n_blocks
            );
        }
        if initial_size < min_heap_size {
            initial_size = min_heap_size;
        }
        let bitmap = SpaceBitmap::empty();
        assert!(min_heap_size <= size as usize);
        Self {
            mark_bitmap: bitmap,
            n_chunks,
            map: mmap,
            free_blocks: free_list,
            reusable_blocks: BlockList::new(),
            chunk_map,
            num_bytes_allocated: AtomicUsize::new(0),
            target_footprint: AtomicUsize::new(initial_size),
            min_heap_size,
            max_heap_size,
            initial_size,
            growth_limit: size as _,
        }
    }
    pub fn init_bitmap(&mut self) {
        self.mark_bitmap = SpaceBitmap::create("immix", self.map.start(), self.map.size());
    }
    pub fn reserved_pages(&self) -> usize {
        self.free_blocks.len() * PAGE_SIZE
    }

    /// Release block by adding it to free list. On Unix platforms it does `madvise` with `MADV_DONTNEED`.
    pub fn release_block(&self, block: *mut ImmixBlock) {
        unsafe {
            (*block).deinit();
            self.map.dontneed(block.cast(), IMMIX_BLOCK_SIZE);
            self.free_blocks.push(block);
        }
    }

    /// Get block from free list and initialize it.
    pub fn get_clean_block(&self) -> *mut ImmixBlock {
        let block = self.free_blocks.pop();
        if block.is_null() {
            return null_mut();
        }
        unsafe {
            (*block).init(false);

            self.chunk_map.set((*block).chunk().cast());

            block
        }
    }

    /// Get first reusable block
    pub fn get_reusable_block(&self) -> *mut ImmixBlock {
        let block = self.reusable_blocks.pop();
        if block.is_null() {
            return null_mut();
        }

        unsafe {
            debug_assert!(self.chunk_map.test((*block).chunk().cast()));
            //(*block).init(false);
            block
        }
    }

    pub fn has_address(&self, ptr: *const u8) -> bool {
        ptr >= self.map.aligned_start() && ptr < self.map.end()
    }
    pub fn object_to_line_num(object: *const u8) -> usize {
        (object as usize % IMMIX_BLOCK_SIZE) / IMMIX_LINE_SIZE
    }
    /// Mark lines for an object. If object is allocated in multiple lines multiple lines are marked.
    pub fn mark_lines(&self, object: *const HeapObjectHeader) {
        unsafe {
            let block = ImmixBlock::align(object.cast()).cast::<ImmixBlock>();
            let chunk = (*block).chunk();
            let size = (*object).size();

            let start = object.cast::<u8>();
            let end = start.add(size);
            let start_line = line_align(start);
            let mut end_line = line_align(end);
            if !is_line_aligned(end) {
                end_line = end_line.add(IMMIX_LINE_SIZE);
            }

            let mut line = start_line;
            while line < end_line {
                (*chunk).line_mark_table().set(line);
                line = line.add(IMMIX_LINE_SIZE);
            }
        }
    }
    /// Prepare for marking phase by settings all blocks state to unamrked and possibly clearing
    /// line mark table if `major_gc` is true.
    pub fn prepare(&self, major_gc: bool) {
        self.chunk_map.visit_marked_range(
            self.map.aligned_start(),
            self.map.end(),
            |chunk| unsafe {
                let chunk = &mut *chunk.cast::<Chunk>();

                for i in 0..CHUNK_BLOCKS {
                    let block = chunk.block(i);
                    if (*block).state() == BlockState::Unallocated {
                        continue;
                    }
                    (*block).set_state(BlockState::Unmarked);
                }
                if major_gc {
                    // Clear marked lines in order for GC to recycle lines properly after GC
                    chunk.line_mark_table_mut().clear_all();
                }
            },
        );
    }

    /// Release dead memory after GC cycle. This function will walk all alive chunks
    /// and sweep allocated blocks in each chunk.
    pub fn release(&self, sweep_color: u8) {
        self.reusable_blocks.reset();
        self.free_blocks.reset();
        self.chunk_map.visit_marked_range(
            self.map.aligned_start(),
            self.map.end(),
            |chunk| unsafe {
                let chunk = chunk.cast::<Chunk>();
                (*chunk).sweep(self, sweep_color);
            },
        );
    }

    pub fn acquire_recyclable_lines(&self, line: *mut u8) -> (*mut u8, *mut u8) {
        ImmixBlock::find_hole(line)
    }
}
