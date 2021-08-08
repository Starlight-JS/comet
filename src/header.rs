use crate::internal::gc_info::GCInfoIndex;
use modular_bitfield::prelude::*;
use std::mem::size_of;

// HeapObjectHeader contains meta data per object and is prepended to each
// object.
//
// +-----------------+------+------------------------------------------+
// | name            | bits |                                          |
// +-----------------+------+------------------------------------------+
// | padding         |   32 | Only present on 64-bit platform.         |
// +-----------------+------+------------------------------------------+
// | GCInfoIndex     |   14 |                                          |
// | unused          |    1 |                                          |
// | unused          |    1 | In construction encoded as |false|.      |
// +-----------------+------+------------------------------------------+
// | size            |   13 | 17 bits because allocations are aligned. |
// | unused          |    1 |                                          |
// | cell state      |    2 |                                          |
// +-----------------+------+------------------------------------------+
//
// Notes:
// - See [GCInfoTable] for constraints on GCInfoIndex.
// - |size| for regular objects is encoded with 14 bits but can actually
//   represent sizes up to |kBlinkPageSize| (2^17) because allocations are
//   always 8 byte aligned (see kAllocationGranularity).
// - |size| for large objects is encoded as 0. The size of a large object is
//   stored in |LargeObjectPage::PayloadSize()|.
// - |mark bit| and |in construction| bits are located in separate 16-bit halves
//    to allow potentially accessing them non-atomically.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct HeapObjectHeader {
    #[cfg(target_pointer_width = "64")]
    _padding: u32,
    encoded_high: EncodedHigh,
    encoded_low: u16,
}

pub const ALLOCATION_GRANULARITY: usize = size_of::<usize>();

impl HeapObjectHeader {
    pub fn is_precise(&self) -> bool {
        self.get_size() == 0
    }
    pub fn set_free(&mut self) {
        self.encoded_low = 0;
    }

    #[inline(always)]
    pub fn from_object(obj: *const u8) -> *mut Self {
        (obj as usize - size_of::<Self>()) as _
    }
    #[inline(always)]
    pub fn payload(&self) -> *const u8 {
        (self as *const Self as usize + size_of::<Self>()) as _
    }
    #[inline(always)]
    pub fn get_gc_info_index(&self) -> GCInfoIndex {
        debug_assert!(
            self.encoded_low > 0,
            "Trying to access non-allocated header {:p} (idx {})",
            self,
            self.encoded_low
        );
        GCInfoIndex(self.encoded_low)
    }
    /// Returns size of an object. If it is allocated in large object space `0` is returned.
    #[inline(always)]
    pub fn get_size(self) -> usize {
        let size = self.encoded_high.size();
        size as usize * ALLOCATION_GRANULARITY
    }
    #[inline(always)]
    pub fn set_size(&mut self, size: usize) {
        self.encoded_high
            .set_size((size / ALLOCATION_GRANULARITY) as _);
    }
    #[inline(always)]
    pub fn is_grey(self) -> bool {
        self.encoded_high.state() == CellState::PossiblyGrey
    }
    #[inline(always)]
    pub fn is_white(self) -> bool {
        self.encoded_high.state() == CellState::DefinitelyWhite
    }
    #[inline(always)]
    pub fn is_black(self) -> bool {
        self.encoded_high.state() == CellState::PossiblyBlack
    }

    #[inline(always)]
    pub fn set_state(&mut self, current: CellState, new: CellState) -> bool {
        if self.encoded_high.state() != current {
            return false;
        }
        self.encoded_high.set_state(new);
        debug_assert_eq!(self.state(), new);
        true
    }
    #[inline(always)]
    pub fn force_set_state(&mut self, state: CellState) {
        self.encoded_high.set_state(state);
    }
    #[inline(always)]
    pub fn set_gc_info(&mut self, index: GCInfoIndex) {
        self.encoded_low = index.0;
    }
    #[inline(always)]
    pub fn is_free(&self) -> bool {
        self.get_gc_info_index().0 == 0
    }

    #[inline]
    pub fn state(&self) -> CellState {
        self.encoded_high.state()
    }
}

#[bitfield(bits = 16)]
#[derive(Clone, Copy)]
pub struct EncodedHigh {
    size: B13,
    #[allow(dead_code)]
    pinned: B1,
    #[bits = 2]
    state: CellState,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, BitfieldSpecifier)]
#[bits = 2]
pub enum CellState {
    DefinitelyWhite,
    PossiblyGrey,
    PossiblyBlack,
}
