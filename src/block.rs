use std::{mem::size_of, ptr::null_mut};

use crate::{
    gc_info_table::GC_TABLE,
    gc_size,
    globals::{IMMIX_BLOCK_SIZE, LINE_COUNT, LINE_SIZE},
    heap::Heap,
    internal::space_bitmap::SpaceBitmap,
};

/// A block is a page-aligned container for heap-allocated objects.
#[repr(C, align(16))]
pub struct Block {
    pub all_next: *mut Self,
    pub next: *mut Self,
    /// Stores magic number to check if block is allocated.
    pub allocated: u32,
    pub hole_count: u8,
    pub heap: *mut Heap,
}

impl Block {
    pub fn set_allocated(&mut self) {
        self.allocated = 0xdeadbeef;
    }
    pub fn offset(&self, offset: usize) -> *mut u8 {
        let x = self as *const Self as usize + offset;
        debug_assert!(
            x >= self.begin() as usize && x <= self.end() as usize,
            "overflow {:x} (end={:p})",
            x,
            self.end()
        );
        x as _
    }
    /// Convert an address on this block into a line number.
    pub fn object_to_line_num(object: *const u8) -> usize {
        (object as usize % IMMIX_BLOCK_SIZE) / LINE_SIZE
    }
    /// Get pointer to block from `object` pointer.
    ///
    /// # Safety
    /// Does not do anything unsafe but might return wrong pointer
    pub unsafe fn get_block_ptr(object: *const u8) -> *mut Self {
        let off = object as usize % IMMIX_BLOCK_SIZE;
        (object as *mut u8).offset(-(off as isize)) as *mut Block
    }

    pub fn new(at: *mut u8) -> &'static mut Self {
        unsafe {
            let ptr = at as *mut Self;
            debug_assert!(ptr as usize % IMMIX_BLOCK_SIZE == 0);
            ptr.write(Self {
                all_next: null_mut(),
                next: null_mut(),
                allocated: 0,
                heap: null_mut(),
                hole_count: LINE_COUNT as _,
            });

            &mut *ptr
        }
    }
    pub fn begin(&self) -> *mut u8 {
        debug_assert!(size_of::<Self>() < LINE_SIZE);
        (self as *const Self as usize + LINE_SIZE) as _
    }

    pub fn end(&self) -> *mut u8 {
        (self as *const Self as usize + IMMIX_BLOCK_SIZE) as _
    }
    #[inline]
    pub fn is_in_block(&self, p: *const u8) -> bool {
        //if self.allocated == 0xdeadbeef {
        let b = self.begin() as usize;
        let e = self.end() as usize;
        b <= p as usize && p as usize <= e
        //} else {
        //    false
        //}
    }
    /// Update the line counter for the given object.
    ///
    /// Mark if `MARK`, otherwise clear mark bit.
    pub fn update_lines<const MARK: bool>(
        &self,
        bitmap: &SpaceBitmap<LINE_SIZE>,
        object: *const u8,
    ) {
        // This calculates how many lines are affected starting from a
        // LINE_SIZE aligned address. So it might not mark enough lines. But
        // that does not matter as we always skip a line in scan_block()
        let line_num = Self::object_to_line_num(object);
        let size = gc_size(object.cast());
        debug_assert!(self.is_in_block(object));

        for line in line_num..(line_num + (size / LINE_SIZE) + 1) {
            debug_assert!(line != 0);
            if MARK {
                bitmap.set(self.line(line));
            } else {
                bitmap.clear(self.line(line));
            }
        }
    }
    pub fn line(&self, index: usize) -> *mut u8 {
        let line = self.offset(index * LINE_SIZE);
        debug_assert!(
            line >= self.begin() && line <= self.end(),
            "invalid line: {:p} (begin={:p},end={:p})",
            line,
            self.begin(),
            self.end()
        );
        line
    }

    pub fn sweep<const MAJOR: bool>(
        &mut self,
        bitmap: &SpaceBitmap<8>,
        live: &SpaceBitmap<8>,
        line: &SpaceBitmap<LINE_SIZE>,
    ) -> SweepResult {
        let mut empty = true;
        let mut count = 0;

        live.visit_marked_range(self.begin(), self.end(), |object| unsafe {
            if !bitmap.test(object as _) {
                if let Some(callback) = GC_TABLE.get_gc_info((*object).get_gc_info_index()).finalize
                {
                    callback((*object).payload() as _);
                }
                live.clear(object as _);

                count += 1;
            } else {
                self.update_lines::<true>(line, object as _);
                bitmap.clear(object as _);
                empty = false;
            }
        });
        if empty {
            SweepResult::Empty
        } else {
            SweepResult::Reuse
        }
    }

    pub fn count_holes(&mut self, bitmap: &SpaceBitmap<LINE_SIZE>) -> usize {
        let mut count = 0;
        for line in 1..LINE_COUNT {
            if !bitmap.test(self.line(line)) {
                count += 1;
            }
        }
        self.hole_count = count as _;
        count
    }

    /// Scan the block for a hole to allocate into.
    ///
    /// The scan will start at `last_high_offset` bytes into the block and
    /// return a tuple of `low_offset`, `high_offset` as the lowest and
    /// highest usable offsets for a hole.
    ///
    /// `None` is returned if no hole was found.
    pub fn scan_block(
        &self,
        bitmap: &SpaceBitmap<LINE_SIZE>,
        last_high_offset: u16,
    ) -> Option<(u16, u16)> {
        let last_high_index = last_high_offset as usize / LINE_SIZE;
        // search for first unmarked line
        let mut low_index = LINE_COUNT - 1;
        for index in (last_high_index + 1)..LINE_COUNT {
            if !bitmap.test(self.line(index)) {
                low_index = index + 1;
                break;
            }
        }
        // search for first marked line
        let mut high_index = LINE_COUNT;
        for index in low_index..LINE_COUNT {
            if bitmap.test(self.line(index)) {
                high_index = index;
                break;
            }
        }
        if low_index == high_index && high_index != (LINE_COUNT - 1) {
            return self.scan_block(bitmap, (high_index * LINE_SIZE - 1) as u16);
        } else if low_index < (LINE_COUNT - 1) {
            return Some((
                (low_index * LINE_SIZE) as u16,
                (high_index * LINE_SIZE - 1) as u16,
            ));
        }
        None
    }

    pub fn init(&mut self, heap: *mut Heap) {
        unsafe {
            (*heap)
                .global
                .line_bitmap
                .clear_range(self.begin(), self.end());
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SweepResult {
    Empty,

    Reuse,
}
