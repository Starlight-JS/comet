//! # Comet
//!
//! Comet is general purpose GC library for Rust. Main usage target for this library is for VM implementated in Rust programming language but nothing stops you from using it
//! in regular Rust code.
//!
//! # Features
//! - Multiple GC policies built-in
//! - Support for multiple threads to allocate into GC heap
//! - Support for multiple GC heaps in one process
//! - Easy to use Rooting API without large runtime overhead like in rust-gc or others.
//!
//!
//!
//! ## GC Policies
//!
//! Comet includes a few GC policies implementations. Each GC policy has its own heap layout and allocation strategy.
//! Here's the list of all GC policies with links to documentation for them:
//! - [Immix](immix)
//! - [MarkSweep](marksweep)
//! - [MiniMark](minimark)
//! - [Semispace](semispace)
//! - [Shenandoah](shenandoah) (NOTE: Very W.I.P & TBD)

#![feature(
    new_uninit,
    const_type_id,
    vec_retain_mut,
    thread_local,
    associated_type_defaults
)]
#[macro_use]
pub mod shadow_stack;
#[macro_use]
pub mod utils;
#[macro_use]
pub mod alloc;
pub mod api;
#[macro_use]
pub mod bitmap;
pub mod bump_pointer_space;
pub mod card_table;
pub mod cms;
pub mod gc_base;
pub mod global;
pub mod immix;
pub mod large_space;
pub mod marksweep;
pub mod minimark;
pub mod mutator;
pub mod rosalloc_space;
pub mod safepoint;
pub mod semispace;
pub mod shenandoah;
pub mod space;
pub mod sticky_immix;
pub mod tlab;
pub mod waitlists;
use std::any::TypeId;

pub use mopa;

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
