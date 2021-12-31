use super::*;
pub const IMMIX_BLOCK_SIZE: usize = 32 * 1024;
pub const IMMIX_LINE_SIZE: usize = 256;
pub const IMMIX_LINES_PER_BLOCK: usize = IMMIX_BLOCK_SIZE / IMMIX_LINE_SIZE;
/// The block allocation state.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BlockState {
    Unallocated,
    Unmarked,
    Marked,
    Reusable { unavailable_lines: u8 },
}

#[repr(C)]
pub struct ImmixBlock {
    next: *mut Self,
    state: BlockState,
    hole_count: u32,
    fragmented: bool,
}

impl ImmixBlock {
    pub fn state(&self) -> BlockState {
        self.state
    }
    pub fn set_state(&mut self, state: BlockState) {
        self.state = state;
    }
    pub fn chunk(&self) -> *mut Chunk {
        Chunk::align(self.start()).cast()
    }

    pub fn deinit(&mut self) {
        self.state = BlockState::Unallocated;
    }
    pub fn init(&mut self, copy: bool) {
        self.state = if copy {
            BlockState::Marked
        } else {
            BlockState::Unmarked
        };
        self.hole_count = 0;
        self.fragmented = false;
        self.next = null_mut();
    }
    pub fn next_atomic(&self) -> &AtomicPtr<ImmixBlock> {
        unsafe { std::mem::transmute(&self.next) }
    }

    pub fn next(&self) -> *mut ImmixBlock {
        self.next
    }

    pub fn set_next(&mut self, block: *mut ImmixBlock) {
        self.next = block;
    }
    pub fn start(&self) -> *mut u8 {
        self as *const Self as _
    }

    pub fn end(&self) -> *mut u8 {
        (self as *const Self as usize + IMMIX_BLOCK_SIZE) as _
    }
    pub fn line(&self, index: u8) -> *mut u8 {
        unsafe { self.start().add(index as usize * IMMIX_LINE_SIZE) }
    }

    pub fn reset(&mut self) {
        self.fragmented = false;
        self.hole_count = 1;
    }

    pub fn is_fragmented(&self) -> bool {
        self.fragmented
    }

    pub fn holes(&self) -> usize {
        self.hole_count as _
    }
    pub fn start_address(&self) -> *mut u8 {
        self.line(1)
    }

    pub fn end_address(&self) -> *mut u8 {
        self.end()
    }

    pub fn find_hole(search_start: *mut u8) -> (*mut u8, *mut u8) {
        unsafe {
            let block = Self::align(search_start).cast::<Self>();
            let chunk = (*block).chunk();
            let line_mark_bitmap = (*chunk).line_mark_table();
            let start_cursor =
                (search_start as usize - (*block).start() as usize) / IMMIX_LINE_SIZE;

            let mut cursor = start_cursor;
            while cursor < IMMIX_LINES_PER_BLOCK {
                let mark = line_mark_bitmap.test((*block).line(cursor as _));
                if !mark {
                    break;
                }
                cursor += 1;
            }
            if cursor == IMMIX_LINES_PER_BLOCK {
                return (null_mut(), null_mut());
            }

            let start = search_start.add((cursor - start_cursor) << 8);
            while cursor < IMMIX_LINES_PER_BLOCK {
                if line_mark_bitmap.test((*block).line(cursor as _)) {
                    break;
                }
                cursor += 1;
            }
            let end = search_start.add((cursor - start_cursor) << 8);
            (start, end)
        }
    }

    pub fn align(addr: *const u8) -> *mut u8 {
        align_down(addr as _, IMMIX_BLOCK_SIZE) as _
    }

    pub fn sweep(&mut self, space: &ImmixSpace) -> bool {
        if self.state == BlockState::Unallocated {
            return true;
        }
        let chunk = self.chunk();
        let line_mark_table = unsafe { (&*chunk).line_mark_table() };
        let mut marked_lines = 0;

        for i in 1..IMMIX_LINES_PER_BLOCK {
            if line_mark_table.test(self.line(i as _)) {
                marked_lines += 1;
            }
        }
        if marked_lines == 0 {
            space.release_block(self as *mut Self);

            true
        } else {
            space
                .num_bytes_allocated
                .fetch_add(marked_lines * IMMIX_LINE_SIZE, Ordering::Relaxed);

            if marked_lines != IMMIX_LINES_PER_BLOCK - 1 {
                self.state = BlockState::Reusable {
                    unavailable_lines: marked_lines as u8,
                };

                space.reusable_blocks.push(self as *mut Self);
            } else {
                self.state = BlockState::Unmarked;
            }
            false
        }
    }
}

impl ImmixBlock {
    pub fn from_object(object: *const u8) -> *mut Self {
        unsafe {
            let offset = object as usize % IMMIX_BLOCK_SIZE;
            let block = object.offset(-(offset as isize));
            block as *mut Self
        }
    }
}

/// A non-block single-linked list to store blocks.
#[derive(Default)]
pub struct BlockList {
    head: AtomicPtr<ImmixBlock>,
    count: AtomicUsize,
}

impl BlockList {
    pub fn new() -> Self {
        Self {
            head: AtomicPtr::new(null_mut()),
            count: AtomicUsize::new(0),
        }
    }
    /// Get number of blocks in this list.
    #[inline]
    pub fn len(&self) -> usize {
        self.count.load(atomic::Ordering::Relaxed)
    }

    /// Add a block to the list.
    #[inline]
    pub fn push(&self, block: *mut ImmixBlock) {
        let mut head = self.head.load(atomic::Ordering::Relaxed);
        loop {
            let new_head = block;
            unsafe {
                (*block).set_next(head);
            }

            match self.head.compare_exchange_weak(
                head,
                new_head,
                atomic::Ordering::SeqCst,
                atomic::Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.count.fetch_add(1, Ordering::AcqRel);
                    return;
                }
                Err(val) => head = val,
            }
        }
    }

    /// Pop a block out of the list.
    #[inline]
    pub fn pop(&self) -> *mut ImmixBlock {
        let mut head = self.head.load(atomic::Ordering::Relaxed);

        loop {
            if head.is_null() {
                return null_mut();
            }
            std::sync::atomic::fence(atomic::Ordering::SeqCst);
            let new_head = unsafe { (*head).next_atomic().load(atomic::Ordering::Acquire) };
            match self.head.compare_exchange_weak(
                head,
                new_head,
                atomic::Ordering::SeqCst,
                atomic::Ordering::Relaxed,
            ) {
                Ok(head) => {
                    self.count.fetch_sub(1, Ordering::AcqRel);
                    return head;
                }
                Err(prev) => head = prev,
            }
        }
    }

    /// Clear the list.
    #[inline]
    pub fn reset(&self) {
        self.head.store(null_mut(), Ordering::Relaxed);
    }

    /// Get an array of all reusable blocks stored in this BlockList.
    #[inline]
    pub fn get_blocks(&self) -> &AtomicPtr<ImmixBlock> {
        &self.head
    }

    #[inline]
    pub fn iter(&self) -> BlockIterator {
        BlockIterator {
            head: self.head.load(Ordering::Acquire),
        }
    }
}

pub struct BlockIterator {
    head: *mut ImmixBlock,
}

impl Iterator for BlockIterator {
    type Item = *mut ImmixBlock;
    fn next(&mut self) -> Option<Self::Item> {
        if self.head.is_null() {
            return None;
        }
        unsafe {
            let prev = self.head;
            self.head = (*prev).next();

            Some(prev)
        }
    }
}
