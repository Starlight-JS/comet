use std::{
    cell::Cell,
    mem::size_of,
    ops::{Deref, DerefMut},
    ptr::null_mut,
};

use crate::{
    api::HeapObjectHeader, bitmap::round_up, space::ContinuousMemMapAllocSpace, util::mmap::Mmap,
};

#[repr(C)]
pub struct BumpPointerSpace {
    space: ContinuousMemMapAllocSpace,

    growth_end: *mut u8,

    main_block_size: Cell<usize>,
    num_blocks: Cell<usize>,
}

impl BumpPointerSpace {
    pub fn contains(&self, obj: *const u8) -> bool {
        obj >= self.begin() && obj < self.limit()
    }

    pub fn can_move_objects(&self) -> bool {
        true
    }
    pub fn is_empty(&self) -> bool {
        self.begin() == self.end()
    }
    /// The total amount of memory reserved for the space.
    pub fn non_growth_limit_capacity(&self) -> usize {
        self.get_mem_map().size()
    }
    /// Limited capacity
    pub fn capacity(&self) -> usize {
        self.growth_end as usize - self.begin() as usize
    }

    pub fn clear_growth_limit(&mut self) {
        self.growth_end = self.limit();
    }

    pub fn create(_name: &'static str, mut capacity: usize) -> Self {
        capacity = round_up(capacity as _, 4096) as usize;
        let mem_map = Mmap::new(capacity);
        let begin = mem_map.start();
        let end = mem_map.end();
        mem_map.commit(begin, end as usize - begin as usize);
        Self {
            space: ContinuousMemMapAllocSpace::new(_name, mem_map, begin, begin, end),
            growth_end: end,
            num_blocks: Cell::new(0),
            main_block_size: Cell::new(0),
        }
    }
    #[inline(always)]
    pub unsafe fn alloc_thread_unsafe(
        &mut self,
        num_bytes: usize,
        bytes_allocated: &mut usize,
        usable_size: &mut usize,
    ) -> *mut HeapObjectHeader {
        let end = self.end.load(atomic::Ordering::Relaxed);
        if end as usize + num_bytes > self.growth_end as usize {
            return null_mut();
        }
        let obj = end.cast::<HeapObjectHeader>();
        *bytes_allocated = num_bytes;
        self.end
            .store((end as usize + num_bytes) as _, atomic::Ordering::Relaxed);
        *usable_size = num_bytes;

        obj
    }
    /// Release the pages back to the operating system
    pub fn clear(&mut self) {
        self.get_mem_map()
            .decommit(self.begin(), self.limit() as usize - self.begin() as usize);
        self.set_end(self.begin());
        self.growth_end = self.limit();
        {
            self.num_blocks.set(0);
            self.main_block_size.set(0);
        }
    }

    pub fn update_main_block(&self) {
        self.main_block_size.set(self.size());
    }

    pub fn alloc_block(&self, mut bytes: usize) -> *mut u8 {
        bytes = round_up(bytes as _, 8) as _;
        if self.num_blocks.get() == 0 {
            self.update_main_block();
        }
        let mut storage = self
            .alloc_non_virtual_without_accounting(bytes + size_of::<BlockHeader>())
            .cast::<u8>();
        if !storage.is_null() {
            unsafe {
                let header = storage as *mut BlockHeader;
                (*header).size = bytes;
                storage = storage.add(size_of::<BlockHeader>());
            }
        }
        storage
    }
    #[inline(always)]
    pub fn alloc_non_virtual_without_accounting(&self, num_bytes: usize) -> *mut HeapObjectHeader {
        let mut old_end;
        let mut new_end;
        while {
            old_end = self.end.load(atomic::Ordering::Relaxed);
            new_end = unsafe { old_end.add(num_bytes) };
            if new_end > self.growth_end {
                return null_mut();
            }
            self.end
                .compare_exchange_weak(
                    old_end,
                    new_end,
                    atomic::Ordering::SeqCst,
                    atomic::Ordering::Relaxed,
                )
                .is_err()
        } {}
        debug_assert!(
            is_aligned(old_end as usize, 16),
            "unaligned pointer {:p} {}",
            old_end,
            num_bytes
        );
        old_end.cast()
    }
}

#[repr(C, align(16))]
struct BlockHeader {
    /// Size of the block in bytes, does not include the header.
    size: usize,
    /// Enusres alignment of MIN_ALLOCATION
    unused: usize,
}
impl Deref for BumpPointerSpace {
    type Target = ContinuousMemMapAllocSpace;
    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.space
    }
}

impl DerefMut for BumpPointerSpace {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.space
    }
}

/// rounds the given value `val` up to the nearest multiple
/// of `align`
pub fn align(value: u32, align: u32) -> u32 {
    if align == 0 {
        return value;
    }

    ((value + align - 1) / align) * align
}

/// rounds the given value `val` up to the nearest multiple
/// of `align`
pub fn align_i32(value: i32, align: i32) -> i32 {
    if align == 0 {
        return value;
    }

    ((value + align - 1) / align) * align
}

/// rounds the given value `val` up to the nearest multiple
/// of `align`.
#[inline(always)]
pub const fn align_usize(value: usize, align: usize) -> usize {
    ((value.wrapping_add(align).wrapping_sub(1)).wrapping_div(align)).wrapping_mul(align)
    //((value + align - 1) / align) * align
}

/// returns 'true' if th given `value` is already aligned
/// to `align`.
pub fn is_aligned(value: usize, align: usize) -> bool {
    value & align.wrapping_sub(1) == 0
}
