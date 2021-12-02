use std::ops::{Deref, DerefMut};

use atomic::Atomic;

use crate::{api::HeapObjectHeader, util::mmap::Mmap};

#[repr(C)]
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
    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.space
    }
}

impl DerefMut for MemMapSpace {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.space
    }
}

#[repr(C)]
pub struct ContinuousMemMapAllocSpace {
    space: MemMapSpace,
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
            space: MemMapSpace::new(name, mem_map, begin, end, limit),
        }
    }

    pub fn create(name: &'static str, size: usize, capacity: usize) -> Self {
        assert!(size <= capacity);
        let mmap = Mmap::new(capacity);

        let start = mmap.start();
        let end = unsafe { start.add(size) };
        let limit = unsafe { start.add(capacity) };
        Self::new(name, mmap, start, end, limit)
    }

    pub fn sweep(&self, _swap_bitmaps: bool) {
        unreachable!()
    }
}

impl Deref for ContinuousMemMapAllocSpace {
    type Target = MemMapSpace;
    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.space
    }
}

impl DerefMut for ContinuousMemMapAllocSpace {
    #[inline(always)]
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
