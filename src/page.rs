use std::mem::size_of;

use crate::{
    api::{HeapObjectHeader, MIN_ALLOCATION},
    bitmap::round_up,
    bump_pointer_space::align_usize,
    util::mmap::Mmap,
};

pub const PAGE_SIZE_LOG2: usize = 17;
pub const PAGE_SIZE: usize = 1 << PAGE_SIZE_LOG2;
pub const PAGE_OFFSET_MASK: usize = PAGE_SIZE_LOG2 - 1;
pub const PAGE_BASE_MASK: usize = !PAGE_OFFSET_MASK;

pub struct Page {
    #[allow(dead_code)]
    mmap: Mmap,
}

impl Page {
    pub fn from_payload(payload: *mut u8) -> *mut Self {
        ((payload as usize & PAGE_BASE_MASK) + 4096) as _
    }
    pub fn payload_start(&self) -> *mut u8 {
        unsafe {
            let addr = (self as *const Self).add(1) as usize;
            align_usize(addr, MIN_ALLOCATION) as _
        }
    }
    pub fn payload_size() -> usize {
        let header_size = round_up(size_of::<Self>() as _, MIN_ALLOCATION as _) as usize;
        PAGE_SIZE - 2 * 4096 - header_size
    }
    pub fn payload_end(&self) -> *mut u8 {
        unsafe { self.payload_start().add(Self::payload_size()) }
    }

    pub fn create() -> *mut Self {
        let mmap = Mmap::new(PAGE_SIZE);
        unsafe {
            let mem = mmap.start().cast::<Self>();
            mem.write(Self { mmap });
            let hdr = (*mem).payload_start().cast::<HeapObjectHeader>();
            (*hdr).set_free();
            (*hdr).set_size(Self::payload_size());
            mem
        }
    }
    pub fn destroy(page: *mut Self) {
        unsafe {
            core::ptr::drop_in_place(page);
        }
    }
}
