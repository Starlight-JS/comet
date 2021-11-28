use std::ops::{Deref, DerefMut};

use atomic::Atomic;

use crate::{
    api::{HeapObjectHeader, MIN_ALLOCATION},
    bitmap::SpaceBitmap,
    util::mmap::Mmap,
};

pub struct ContinuousSpace {
    name: &'static str,
    begin: *mut u8,
    pub(crate) end: Atomic<*mut u8>,
    limit: *mut u8,
}

impl ContinuousSpace {
    pub fn new(name: &'static str, begin: *mut u8, end: *mut u8, limit: *mut u8) -> Self {
        Self {
            name,
            begin,
            end: Atomic::new(end),
            limit,
        }
    }
    pub fn has_address(&self, obj: *const HeapObjectHeader) -> bool {
        obj >= self.begin as *const _ && obj < self.limit as *const _
    }

    pub fn contains(&self, obj: *const HeapObjectHeader) -> bool {
        self.has_address(obj)
    }
    pub fn end(&self) -> *mut u8 {
        self.end.load(atomic::Ordering::Relaxed)
    }

    pub fn begin(&self) -> *mut u8 {
        self.begin
    }

    pub fn limit(&self) -> *mut u8 {
        self.limit
    }

    pub fn size(&self) -> usize {
        self.end() as usize - self.begin() as usize
    }

    pub fn set_end(&self, end: *mut u8) {
        self.end.store(end, atomic::Ordering::Relaxed);
    }

    pub fn set_limit(&mut self, limit: *mut u8) {
        self.limit = limit;
    }

    pub fn capacity(&self) -> usize {
        self.limit() as usize - self.begin() as usize
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

pub struct MemMapSpace {
    mmap: Mmap,
    space: ContinuousSpace,
}

impl MemMapSpace {
    pub fn new(
        name: &'static str,
        mem_map: Mmap,
        begin: *mut u8,
        end: *mut u8,
        limit: *mut u8,
    ) -> Self {
        Self {
            mmap: mem_map,
            space: ContinuousSpace::new(name, begin, end, limit),
        }
    }

    pub fn get_mem_map(&self) -> &Mmap {
        &self.mmap
    }
}

impl Deref for MemMapSpace {
    type Target = ContinuousSpace;
    fn deref(&self) -> &Self::Target {
        &self.space
    }
}

impl DerefMut for MemMapSpace {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.space
    }
}

pub struct ContinuousMemMapAllocSpace {
    space: MemMapSpace,
    live_bitmap: SpaceBitmap<{ MIN_ALLOCATION }>,
    mark_bitmap: SpaceBitmap<{ MIN_ALLOCATION }>,
    temp_bitmap: SpaceBitmap<{ MIN_ALLOCATION }>,
    finalize_bitmap: SpaceBitmap<{ MIN_ALLOCATION }>,
}

impl ContinuousMemMapAllocSpace {
    pub fn new(
        name: &'static str,
        mem_map: Mmap,
        begin: *mut u8,
        end: *mut u8,
        limit: *mut u8,
    ) -> Self {
        Self {
            finalize_bitmap: SpaceBitmap::create(
                "finalize-bitmap",
                begin,
                limit as usize - begin as usize,
            ),
            space: MemMapSpace::new(name, mem_map, begin, end, limit),
            live_bitmap: SpaceBitmap::empty(),
            mark_bitmap: SpaceBitmap::empty(),
            temp_bitmap: SpaceBitmap::empty(),
        }
    }
    pub fn get_finalize_bitmap(&self) -> &SpaceBitmap<{ MIN_ALLOCATION }> {
        &self.finalize_bitmap
    }
    pub fn get_temp_bitmap_mut(&mut self) -> &mut SpaceBitmap<{ MIN_ALLOCATION }> {
        &mut self.temp_bitmap
    }

    pub fn get_live_bitmap_mut(&mut self) -> &mut SpaceBitmap<{ MIN_ALLOCATION }> {
        &mut self.live_bitmap
    }

    pub fn get_mark_bitmap_mut(&mut self) -> &mut SpaceBitmap<{ MIN_ALLOCATION }> {
        &mut self.mark_bitmap
    }

    pub fn get_temp_bitmap(&self) -> &SpaceBitmap<{ MIN_ALLOCATION }> {
        &self.temp_bitmap
    }

    pub fn get_live_bitmap(&self) -> &SpaceBitmap<{ MIN_ALLOCATION }> {
        &self.live_bitmap
    }

    pub fn get_mark_bitmap(&self) -> &SpaceBitmap<{ MIN_ALLOCATION }> {
        &self.mark_bitmap
    }

    pub fn create(name: &'static str, size: usize, capacity: usize) -> Self {
        assert!(size <= capacity);
        let mmap = Mmap::new(capacity + 16);

        let start = mmap.start();
        let end = unsafe { start.add(size) };
        let limit = unsafe { start.add(capacity) };
        Self::new(name, mmap, start, end, limit)
    }

    pub fn bind_bitmaps(&mut self) {
        self.mark_bitmap = SpaceBitmap::create("mark-bitmap", self.begin(), self.capacity());
        self.live_bitmap = SpaceBitmap::create("live-bitmap", self.begin(), self.capacity());
    }

    pub fn swap_bitmaps(&mut self) {
        std::mem::swap(&mut self.mark_bitmap, &mut self.live_bitmap);
    }

    pub fn has_bound_bitmaps(&self) -> bool {
        self.mark_bitmap.begin() == self.live_bitmap.begin()
    }

    pub fn unbind_bitmaps(&mut self) {
        self.mark_bitmap = std::mem::replace(&mut self.temp_bitmap, SpaceBitmap::empty());
    }

    pub fn bind_live_to_mark_bitmap(&mut self) {
        self.temp_bitmap = std::mem::replace(&mut self.mark_bitmap, SpaceBitmap::empty());
        self.mark_bitmap.copy_view(&self.live_bitmap);
    }

    pub fn sweep(&self, _swap_bitmaps: bool) {
        unreachable!()
    }
}

impl Deref for ContinuousMemMapAllocSpace {
    type Target = MemMapSpace;
    fn deref(&self) -> &Self::Target {
        &self.space
    }
}

impl DerefMut for ContinuousMemMapAllocSpace {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.space
    }
}

impl std::fmt::Debug for ContinuousMemMapAllocSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ContinuousMemMapAllocSpace: {:p}->{:p}(limit {:p}) ",
            self.begin(),
            self.end(),
            self.limit()
        )
    }
}
