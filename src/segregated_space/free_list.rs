use std::ptr::null_mut;

use crate::api::HeapObjectHeader;

pub struct FreeList {
    head: *mut FreeEntry,
    size_class: usize,
}

impl FreeList {
    pub fn new(size: usize) -> Self {
        Self {
            head: null_mut(),
            size_class: size,
        }
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }
    #[inline]
    pub fn take(&mut self) -> *mut FreeEntry {
        unsafe {
            if self.head.is_null() {
                return self.head;
            }

            let head = self.head;
            self.head = (*head).next();
            head
        }
    }
    #[inline]
    pub fn add(&mut self, block: *mut u8) {
        unsafe {
            let entry = FreeEntry::create(block, self.size_class);
            (*entry).set_next(self.head);
            self.head = entry;
        }
    }
}

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct FreeEntry {
    header: HeapObjectHeader,
}

impl FreeEntry {
    #[inline]
    pub fn create(at: *mut u8, size: usize) -> *mut Self {
        unsafe {
            let hdr = at.cast::<HeapObjectHeader>();

            (*hdr).set_size(size);
            (*hdr).set_free();
            hdr.cast()
        }
    }
    pub fn next(self) -> *mut FreeEntry {
        self.header.vtable() as *mut _
    }

    pub fn set_next(&mut self, next: *mut FreeEntry) {
        self.header.set_vtable(next as _);
    }
    #[inline]
    pub fn size(&self) -> usize {
        self.header.size()
    }
}
