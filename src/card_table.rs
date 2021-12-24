use std::mem::size_of;

use crate::utils::mmap::Mmap;

pub const CARD_SIZE: usize = 512;
pub const CARD_SIZE_BITS: usize = 9;
pub const CARD_REFS: usize = CARD_SIZE / size_of::<usize>();

#[allow(dead_code)]
pub struct CardTable {
    start: *mut u8,
    end: *mut u8,
    map: Mmap,
    heap_begin: *mut u8,
    heap_size: usize,
}
