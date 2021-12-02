#![feature(core_intrinsics, const_type_id)]
#[macro_use]
pub mod util;
#[macro_use]
pub mod api;
pub mod bitmap;
use std::{any::TypeId, ptr::null_mut};

use api::{Collectable, Gc};
pub use mopa;
pub mod alloc;
pub mod base;
pub mod bump_pointer_space;
pub mod large_space;
pub mod minimark;
pub mod miri_stack;
pub mod page;
pub mod semispace;
pub mod space;

pub type Heap = semispace::SemiSpace;

pub fn alloc_i32(heap: &mut impl base::GcBase, x: i32) -> Gc<i32> {
    heap.allocate(x)
}

pub fn is_i32(x: Gc<dyn Collectable>) -> bool {
    x.is::<i32>()
}
const FNV_OFFSET_BASIS_32: u32 = 0x811c9dc5;

const FNV_PRIME_32: u32 = 0x01000193;

/// Computes 32-bits fnv1a hash of the given slice, or up-to limit if provided.
/// If limit is zero or exceeds slice length, slice length is used instead.
#[inline(always)]
const fn fnv1a_hash_32(bytes: &[u8], limit: Option<usize>) -> u32 {
    let mut hash = FNV_OFFSET_BASIS_32;

    let mut i = 0;
    let len = match limit {
        Some(v) => {
            if v <= bytes.len() && v > 0 {
                v
            } else {
                bytes.len()
            }
        }
        None => bytes.len(),
    };

    while i < len {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(FNV_PRIME_32);
        i += 1;
    }
    hash
}

/*
/// Computes 32-bits fnv1a hash and XORs higher and lower 16-bits.
/// This results in a 16-bits hash value.
/// Up to limit if provided, otherwise slice length.
/// If limit is zero or exceeds slice length, slice length is used instead.
#[inline(always)]
const fn fnv1a_hash_16_xor(bytes: &[u8], limit: Option<usize>) -> u16 {
    let bytes = fnv1a_hash_32(bytes, limit).to_ne_bytes();
    let upper: u16 = u16::from_ne_bytes([bytes[0], bytes[1]]);
    let lower: u16 = u16::from_ne_bytes([bytes[2], bytes[3]]);
    upper ^ lower
}
*/

#[inline(always)]
pub(crate) const fn small_type_id<T: 'static>() -> u32 {
    unsafe {
        let bytes: [u8; std::mem::size_of::<TypeId>()] = std::mem::transmute(TypeId::of::<T>());
        fnv1a_hash_32(&bytes, None)
    }
}
