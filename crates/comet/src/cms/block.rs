use std::{mem::size_of, ptr::null_mut};

use crate::api::HeapObjectHeader;

pub const BLOCK_SIZE: usize = 16 * 1024;
pub const ATOM_SIZE: usize = 16;
pub const BLOCK_PAYLOAD: usize = BLOCK_SIZE - size_of::<Block>();

pub struct FreeList {
    head: *mut HeapObjectHeader,
}

impl FreeList {
    pub fn new() -> Self {
        Self { head: null_mut() }
    }

    pub fn add(&mut self, entry: *mut u8) {
        unsafe {
            let entry = entry.cast::<HeapObjectHeader>();
            (*entry).set_free();
            (*entry).value.raw = self.head as u64;
            self.head = entry;
        }
    }

    pub fn take(&mut self) -> *mut HeapObjectHeader {
        unsafe {
            let prev = self.head;
            self.head = (*prev).value.raw as *mut HeapObjectHeader;
            prev
        }
    }
}

pub struct Block {
    free_list: FreeList,
    cell_size: u8,
}

impl Block {}
