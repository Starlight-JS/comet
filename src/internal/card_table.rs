use std::{
    mem::size_of,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

use memmap2::MmapMut;
#[inline(always)]
fn byte_cas(old_value: u8, new_value: u8, address: *mut u8) -> bool {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    unsafe {
        let address = address.cast::<AtomicU8>();
        (*address)
            .compare_exchange_weak(old_value, new_value, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    unsafe {
        use std::mem::size_of;
        use std::sync::atomic::AtomicUsize;
        let shift_in_bytes = address as usize % size_of::<usize>();
        let address = address.sub(shift_in_bytes);
        let shift_in_bits = shift_in_bytes * 8;
        let word_atomic = address.cast::<AtomicUsize>();
        let cur_word = word_atomic.load(Ordering::Relaxed) & !(0xff << shift_in_bits);
        let old_word = cur_word | ((old_value as usize) << shift_in_bits);
        let new_word = cur_word | ((new_value as usize) << shift_in_bits);
        word_atomic
            .compare_exchange_weal(old_word, new_Word, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }
}

/// Maintain a card table from the the write barrier. All writes of
/// non-null values to heap addresses should go through an entry in
/// WriteBarrier, and from there to here.
#[allow(dead_code)]
pub struct CardTable {
    /// Mmapped pages for the card table
    mem_map: MmapMut,
    /// Value used to compute card table addresses from object addresses, see get_biased_begin
    biased_begin: *const u8,
    /// Card table doesn't begin at the beginning of the mem_map, instead it is displaced by offset
    /// to allow the byte value of `biased_begin` to equal [CARD_DIRTY](CardTable::CARD_DIRTY).
    offset: usize,
}
impl CardTable {
    pub const CARD_SHIFT: usize = 10;
    pub const CARD_SIZE: usize = 1 << Self::CARD_SHIFT;
    pub const CARD_CLEAN: u8 = 0x0;
    pub const CARD_DIRTY: u8 = 0x70;
    pub const CARD_AGED: u8 = Self::CARD_DIRTY - 1;
    /// Returns a value that when added to a heap address >> [CARD_SHIFT](CardTable::CARD_SHIFT) will address the appropriate
    /// card table byte. For convenience this value is cached in every local heap.
    pub fn get_biased_begin(&self) -> *mut u8 {
        self.biased_begin as _
    }

    pub fn mem_map_begin(&self) -> *mut u8 {
        self.mem_map.as_ptr() as _
    }
    pub fn mem_map_size(&self) -> usize {
        self.mem_map.len()
    }
    #[inline]
    pub fn card_from_addr(&self, addr: *const u8) -> *mut u8 {
        let card_addr = self.biased_begin as usize + (addr as usize >> Self::CARD_SHIFT);
        card_addr as _
    }

    #[inline]
    pub fn addr_from_card(&self, card_addr: *mut u8) -> *mut u8 {
        let offset = card_addr as usize - self.biased_begin as usize;
        (offset << Self::CARD_SHIFT) as _
    }

    #[inline]
    pub fn modify_cards_atomic(
        &self,
        scan_begin: *mut u8,
        scan_end: *mut u8,
        mut visitor: impl FnMut(u8) -> u8,
        mut modified: impl FnMut(*mut u8, u8, u8),
    ) {
        unsafe {
            let mut card_cur = self.card_from_addr(scan_begin);
            let mut card_end = self.card_from_addr(round_up(scan_end as _, Self::CARD_SIZE) as _);

            while !is_aligned(card_cur as _, size_of::<usize>()) && card_cur < card_end {
                let mut expected;
                let mut new_value;
                while {
                    expected = *card_cur;
                    new_value = visitor(expected);
                    expected != new_value && !byte_cas(expected, new_value, card_cur)
                } {}
                if expected != new_value {
                    modified(card_cur, expected, new_value);
                }
                card_cur = card_cur.add(1);
            }
            while !is_aligned(card_end as _, size_of::<usize>()) && card_end > card_cur {
                card_end = card_end.sub(1);
                let mut expected;
                let mut new_value;
                while {
                    expected = *card_end;
                    new_value = visitor(expected);
                    expected != new_value && !byte_cas(expected, new_value, card_cur)
                } {}
                if expected != new_value {
                    modified(card_end, expected, new_value);
                }
            }

            let mut word_cur = card_cur.cast::<usize>();
            let word_end = card_end.cast::<usize>();

            union U1 {
                expected_word: usize,
                expected_bytes: [u8; size_of::<usize>()],
            }

            union U2 {
                new_word: usize,
                new_bytes: [u8; size_of::<usize>()],
            }

            let mut u1 = U1 { expected_word: 0 };
            let mut u2 = U2 { new_word: 0 };
            while word_cur < word_end {
                loop {
                    u1.expected_word = *word_cur;
                    if u1.expected_word == 0 {
                        break; // clean card
                    }
                    for i in 0..size_of::<usize>() {
                        u2.new_bytes[i] = visitor(u1.expected_bytes[i]);
                    }
                    let atomic_word = word_cur.cast::<AtomicUsize>();
                    if (*atomic_word)
                        .compare_exchange_weak(
                            u1.expected_word,
                            u2.new_word,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        for i in 0..size_of::<usize>() {
                            let expected_byte = u1.expected_bytes[i];
                            let new_byte = u2.new_bytes[i];
                            if new_byte != expected_byte {
                                modified(word_cur.cast::<u8>().add(i), expected_byte, new_byte);
                            }
                        }
                        break;
                    }
                }
                word_cur = word_cur.add(1);
            }
        }
    }
}

fn is_aligned(x: usize, n: usize) -> bool {
    (x & (n - 1)) == 0
}

fn round_up(x: usize, n: usize) -> usize {
    round_down(x + n - 1, n)
}

fn round_down(x: usize, n: usize) -> usize {
    x & !n
}
