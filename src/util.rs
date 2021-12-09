#[inline(always)]
pub const fn align_down(addr: usize, align: usize) -> usize {
    addr & !align.wrapping_sub(1)
}
#[inline(always)]
pub const fn align_up(addr: usize, align: usize) -> usize {
    // See https://github.com/rust-lang/rust/blob/e620d0f337d0643c757bab791fc7d88d63217704/src/libcore/alloc.rs#L192
    addr.wrapping_sub(align).wrapping_sub(1) & !align.wrapping_sub(1)
}
#[inline(always)]
pub const fn is_aligned(addr: usize, align: usize) -> bool {
    addr & align.wrapping_sub(1) == 0
}

pub trait BitFieldTrait<const SHIFT: u64, const SIZE: u64> {
    type Next;
    const MASK: u64 = ((1 << SHIFT) << SIZE) - (1 << SHIFT);

    fn encode(value: u64) -> u64 {
        value.wrapping_shl(SHIFT as _)
    }
    fn update(previous: u64, value: u64) -> u64 {
        (previous & !Self::MASK) | Self::encode(value)
    }

    fn decode(value: u64) -> u64 {
        (value & Self::MASK).wrapping_shr(SHIFT as _)
    }
}

pub struct VTableBitField;

pub struct SizeBitField;

pub struct MarkedBitField;

pub struct ForwardedBit;

pub struct ParentKnown;

pub struct Pinned;

impl BitFieldTrait<1, 1> for Pinned {
    type Next = ParentKnown;
}

impl BitFieldTrait<1, 1> for ParentKnown {
    type Next = MarkBit;
}

impl BitFieldTrait<2, 1> for ForwardedBit {
    type Next = MarkedBitField;
}

impl BitFieldTrait<0, 58> for VTableBitField {
    type Next = SizeBitField;
}

impl BitFieldTrait<48, 1> for MarkedBitField {
    type Next = MarkedBitField;
}

impl BitFieldTrait<0, 13> for SizeBitField {
    type Next = MarkedBitField;
}

pub mod bits;
pub mod mmap;
pub mod stack;
pub struct MarkBit;

impl BitFieldTrait<0, 1> for MarkBit {
    type Next = Self;
}
use std::fmt;
pub struct FormattedSize {
    pub size: usize,
}

impl fmt::Display for FormattedSize {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let ksize = (self.size as f64) / 1024f64;

        if ksize < 1f64 {
            return write!(f, "{}B", self.size);
        }

        let msize = ksize / 1024f64;

        if msize < 1f64 {
            return write!(f, "{:.1}K", ksize);
        }

        let gsize = msize / 1024f64;

        if gsize < 1f64 {
            write!(f, "{:.1}M", msize)
        } else {
            write!(f, "{:.1}G", gsize)
        }
    }
}

pub fn formatted_size(size: usize) -> FormattedSize {
    FormattedSize { size }
}
