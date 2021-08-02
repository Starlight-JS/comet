use std::{num::NonZeroU16, ptr::null_mut};

use crate::{
    gc_info_table::GC_TABLE,
    header::HeapObjectHeader,
    internal::{space_bitmap::SpaceBitmap, BLOCK_SIZE},
};

pub struct FreeList {
    pub(super) next: *mut FreeEntry,
}

impl FreeList {
    pub fn new() -> Self {
        Self { next: null_mut() }
    }

    pub fn get(&mut self, size: usize) -> *mut u8 {
        unsafe {
            if self.next.is_null() {
                return null_mut();
            }
            let _ = size;
            #[cfg(feature = "valgrind")]
            unsafe {
                crate::gc::vgrs::memcheck::malloclike_block(self.next as _, size, 0, false);
            }
            let prev = self.next;
            self.next = (*prev).next;
            prev.cast()
        }
    }

    pub fn add(&mut self, ptr: *mut u8) {
        unsafe {
            let ptr = ptr.cast::<FreeEntry>();
            (*ptr).next = self.next;
            (*ptr.cast::<HeapObjectHeader>()).set_free();
            self.next = ptr;
        }
    }
}

#[repr(C)]
pub struct FreeEntry {
    preserved: usize,
    next: *mut Self,
}
// A block is a page-aligned container for heap-allocated objects.
// Objects are allocated within cells of the marked block. For a given
// marked block, all cells have the same size. Objects smaller than the
// cell size may be allocated in the block, in which case the
// allocation suffers from internal fragmentation: wasted space whose
// size is equal to the difference between the cell size and the object
// size.
#[repr(C, align(16))]
pub struct Block {
    pub next: *mut Self,
    pub allocated: u32,

    pub cell_size: NonZeroU16,
    pub freelist: FreeList,

    pub data_start: [u16; 0],
}

impl Block {
    /// Get pointer to block from `object` pointer.
    ///
    /// # Safety
    /// Does not do anything unsafe but might return wrong pointer
    pub unsafe fn get_block_ptr(object: *const u8) -> *mut Self {
        let off = object as usize % BLOCK_SIZE;
        (object as *mut u8).offset(-(off as isize)) as *mut Block
    }

    pub fn new(at: *mut u8) -> &'static mut Self {
        unsafe {
            let ptr = at as *mut Self;
            debug_assert!(ptr as usize % BLOCK_SIZE == 0);
            ptr.write(Self {
                next: null_mut(),
                allocated: 0,

                freelist: FreeList::new(),
                cell_size: NonZeroU16::new_unchecked(u16::MAX),
                data_start: [],
            });

            &mut *ptr
        }
    }
    #[inline]
    pub fn is_in_block(&self, p: *const u8) -> bool {
        if self.allocated == 0xdeadbeef {
            let b = self.begin();
            let e = b + BLOCK_SIZE;
            b < p as usize && p as usize <= e
        } else {
            false
        }
    }

    pub fn allocate(&mut self) -> *mut u8 {
        self.freelist.get(self.cell_size.get() as usize)
    }

    pub fn deallocate(&mut self, ptr: *const u8) {
        self.freelist.add(ptr as _);
    }

    pub fn begin(&self) -> usize {
        self.data_start.as_ptr() as usize
    }

    pub fn end(&self) -> usize {
        self as *const Self as usize + BLOCK_SIZE
    }
    pub fn init(&mut self, cell_size: u16) {
        debug_assert!(cell_size >= 16, "Block cell size should be aligned to 16");
        unsafe {
            self.cell_size = NonZeroU16::new_unchecked(cell_size);
            self.allocated = 0xdeadbeef;
            let mut freelist = FreeList::new();
            self.walk(|cell| {
                freelist.add(cell);
            });
            self.freelist = freelist;
        }
    }

    pub fn walk(&self, mut cb: impl FnMut(*mut u8)) {
        for i in 0..self.cell_count() {
            cb(self.cell(i));
        }
    }

    pub fn cell_count(&self) -> usize {
        (BLOCK_SIZE - offsetof!(Self.data_start)) / self.cell_size.get() as usize
    }

    pub fn cell_from_ptr(&self, ptr: *const u8) -> *mut u8 {
        if ptr < self.begin() as *const u8 {
            return null_mut();
        }

        let index = (ptr as usize - self.begin()) / self.cell_size.get() as usize;

        let end = self.cell_count();

        if index >= end {
            return null_mut();
        }
        self.cell(index)
    }
    pub fn cell_index(&self, ptr: *const u8) -> usize {
        (ptr as usize - self.begin()) / self.cell_size.get() as usize
    }
    pub fn cell(&self, idx: usize) -> *mut u8 {
        (self
            .begin()
            .wrapping_add(idx.wrapping_mul(self.cell_size.get() as usize))) as _
    }
    pub fn offset(&self, offset: usize) -> usize {
        self.begin() + offset
    }

    pub fn sweep(&mut self, bitmap: &SpaceBitmap<16>) -> SweepResult {
        let mut freelist = FreeList::new();
        let mut free = 0;
        let mut empty = true;
        let table = unsafe { &GC_TABLE };
        self.walk(|cell| unsafe {
            let header = cell.cast::<HeapObjectHeader>();

            if (*header).is_free() {
                debug_assert!(!bitmap.test(header.cast()));
                free += 1;
                freelist.add(header.cast());
            } else {
                debug_assert!(bitmap.test(header.cast()));

                if (*header).set_state(
                    crate::header::CellState::PossiblyBlack,
                    crate::header::CellState::DefinitelyWhite,
                ) {
                    empty = false;
                } else {
                    bitmap.clear(header.cast());
                    let info = table.get_gc_info((*header).get_gc_info_index());
                    if let Some(callback) = info.finalize {
                        callback((*header).payload() as _);
                    }
                    free += 1;
                    freelist.add(header.cast());
                }
            }
        });
        if free == 0 {
            SweepResult::Full
        } else if free > 0 && empty {
            SweepResult::Empty
        } else {
            SweepResult::Reusable
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SweepResult {
    Empty,
    Full,
    Reusable,
}
