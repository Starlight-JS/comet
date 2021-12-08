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

impl BitFieldTrait<62, 1> for ForwardedBit {
    type Next = MarkedBitField;
}

impl BitFieldTrait<0, 48> for VTableBitField {
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
