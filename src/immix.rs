use std::{
    ptr::null_mut,
    sync::atomic::{AtomicPtr, AtomicUsize},
};

use crate::{bitmap::SpaceBitmap, utils::align_down};

pub const IMMIX_BLOCK_SIZE: usize = 32 * 1024;
pub const IMMIX_LINE_SIZE: usize = 256;
pub const IMMIX_LINES_PER_BLOCK: usize = IMMIX_BLOCK_SIZE / IMMIX_LINE_SIZE;

#[repr(C)]
pub struct ImmixBlock {
    next: *mut Self,
    allocated: bool,
    unavailable_lines: u8,
    hole_count: u32,
    fragmented: bool,
}

impl ImmixBlock {
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

    pub fn find_hole(
        line_mark_bitmap: &SpaceBitmap<{ IMMIX_LINE_SIZE }>,
        search_start: *mut u8,
    ) -> (*mut u8, *mut u8) {
        unsafe {
            let block = Self::align(search_start).cast::<Self>();
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
                Ok(_) => return,
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
                Ok(head) => return head,
                Err(prev) => head = prev,
            }
        }
    }

    /// Clear the list.
    #[inline]
    pub fn reset(&self) {
        unsafe { &mut *self.head.load(atomic::Ordering::Acquire) }.set_next(null_mut());
    }

    /// Get an array of all reusable blocks stored in this BlockList.
    #[inline]
    pub fn get_blocks(&self) -> &AtomicPtr<ImmixBlock> {
        &self.head
    }
}

pub struct Chunk {
    next: *mut Chunk,
    line_mark_bitmap: SpaceBitmap<{ IMMIX_LINE_SIZE }>,
}

pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;
pub const CHUNK_BLOCKS: usize = CHUNK_SIZE / IMMIX_BLOCK_SIZE;

impl Chunk {
    pub fn new(at: *mut u8) -> *mut Chunk {
        unsafe {
            at.cast::<Self>().write(Self {
                next: null_mut(),
                line_mark_bitmap: SpaceBitmap::create("line mark bitmap", at, CHUNK_SIZE),
            });
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
        unsafe { ImmixBlock::align(self.start().add(IMMIX_BLOCK_SIZE)).cast() }
    }
    pub fn block(&self, index: usize) -> *mut ImmixBlock {
        todo!()
    }
}
