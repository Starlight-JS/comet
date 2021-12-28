use std::sync::atomic::AtomicPtr;

use rosalloc::defs::PAGE_SIZE;

use crate::{
    bitmap::{round_up, SpaceBitmap},
    utils::mmap::Mmap,
};

pub struct Space {
    name: String,
}

impl Space {
    pub fn get_name(&self) -> &str {
        &self.name
    }
}

pub struct ContinuousSpace {
    space: Space,
    begin: *mut u8,
    end: AtomicPtr<u8>,
    limit: *mut u8,
}
deref_impl!(ContinuousSpace;Space where space);

impl ContinuousSpace {
    #[inline]
    pub fn begin(&self) -> *mut u8 {
        self.begin
    }
    #[inline]
    pub fn end(&self) -> *mut u8 {
        self.end.load(atomic::Ordering::Relaxed)
    }
    #[inline]
    pub fn limit(&self) -> *mut u8 {
        self.limit
    }
    #[inline]
    pub fn set_limit(&mut self, limit: *mut u8) {
        self.limit = limit;
    }
    #[inline]
    pub fn set_end(&self, end: *mut u8) {
        self.end.store(end, atomic::Ordering::Relaxed);
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.end() as usize - self.begin() as usize
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.limit() as usize - self.begin() as usize
    }

    #[inline]
    pub fn has_address(&self, obj: *const u8) -> bool {
        obj >= self.begin() && obj < self.limit()
    }

    #[inline]
    pub fn is_continuous_space(&self) -> bool {
        true
    }

    #[inline]
    pub fn new(name: &str, begin: *mut u8, end: *mut u8, limit: *mut u8) -> Self {
        Self {
            limit,
            end: AtomicPtr::new(end),
            begin,
            space: Space {
                name: name.to_string(),
            },
        }
    }
}

pub struct MemMapSpace {
    pub space: ContinuousSpace,
    pub mem_map: Mmap,
}

deref_impl!(MemMapSpace;ContinuousSpace where space);

impl MemMapSpace {
    #[inline]
    pub fn release_mem_map(&mut self) -> Mmap {
        std::mem::replace(&mut self.mem_map, Mmap::uninit())
    }
    #[inline]
    pub fn non_growth_limit_capacity(&self) -> usize {
        self.capacity()
    }

    #[inline]
    pub fn get_mem_map(&self) -> &Mmap {
        &self.mem_map
    }

    #[inline]
    pub fn new(name: &str, mem_map: Mmap, begin: *mut u8, end: *mut u8, limit: *mut u8) -> Self {
        Self {
            space: ContinuousSpace::new(name, begin, end, limit),
            mem_map,
        }
    }
}

pub struct ContinuousMemMapAllocSpace {
    pub space: MemMapSpace,
    pub live_bitmap: SpaceBitmap<8>,
    pub mark_bitmap: SpaceBitmap<8>,
    pub temp_bitmap: SpaceBitmap<8>,
}

deref_impl!(ContinuousMemMapAllocSpace;MemMapSpace where space);

impl ContinuousMemMapAllocSpace {
    pub fn swap_bitmaps(&mut self) {
        std::mem::swap(&mut self.mark_bitmap, &mut self.live_bitmap);
    }

    pub fn sweep_colored(
        &mut self,
        mut callback: impl FnMut(&[*mut u8], bool) -> usize,
        alloc_color: u8,
        mark_color: u8,
    ) -> (usize, usize) {
        unsafe {
            let live_bitmap = &*self.get_live_bitmap();

            let mut freed = 0;
            let mut num_ptrs = 0;

            SpaceBitmap::<8>::sweep_walk_color(
                live_bitmap,
                self.begin() as _,
                self.end() as _,
                |ptrc, ptrs| {
                    let pointers = std::slice::from_raw_parts(ptrs.cast::<*mut u8>(), ptrc);
                    num_ptrs += pointers.len();

                    freed += callback(pointers, false);
                },
                None,
                mark_color,
                alloc_color,
            );

            (freed, num_ptrs)
        }
    }

    pub fn sweep(
        &mut self,
        swap_bitmaps: bool,
        mut callback: impl FnMut(&[*mut u8], bool) -> usize,
    ) -> (usize, usize) {
        unsafe {
            let mut live_bitmap = &*self.get_live_bitmap();
            let mut mark_bitmap = &*self.get_mark_bitmap();
            if live_bitmap as *const _ == mark_bitmap as *const _ {
                return (0, 0);
            }

            if swap_bitmaps {
                std::mem::swap(&mut live_bitmap, &mut mark_bitmap);
            }

            let mut freed = 0;
            let mut num_ptrs = 0;
            SpaceBitmap::<8>::sweep_walk(
                live_bitmap,
                mark_bitmap,
                self.begin() as _,
                self.end() as _,
                |ptrc, ptrs| {
                    let pointers = std::slice::from_raw_parts(ptrs.cast::<*mut u8>(), ptrc);
                    num_ptrs += pointers.len();

                    freed += callback(pointers, swap_bitmaps);
                },
            );
            (freed, num_ptrs)
        }
    }

    #[inline]
    pub fn new(name: &str, mem_map: Mmap, begin: *mut u8, end: *mut u8, limit: *mut u8) -> Self {
        Self {
            space: MemMapSpace::new(name, mem_map, begin, end, limit),
            mark_bitmap: SpaceBitmap::empty(),
            live_bitmap: SpaceBitmap::empty(),
            temp_bitmap: SpaceBitmap::empty(),
        }
    }
    #[inline]
    pub fn allocate_bitmaps(&mut self) {
        self.live_bitmap =
            SpaceBitmap::create("live-bitmap", self.begin(), self.get_mem_map().size());
        self.mark_bitmap =
            SpaceBitmap::create("mark-bitmap", self.begin(), self.get_mem_map().size());
    }
    #[inline]
    pub fn get_temp_bitmap(&self) -> *mut SpaceBitmap<8> {
        &self.temp_bitmap as *const _ as *mut _
    }

    #[inline]
    pub fn get_mark_bitmap(&self) -> *mut SpaceBitmap<8> {
        &self.mark_bitmap as *const _ as *mut _
    }
    #[inline]
    pub fn get_live_bitmap(&self) -> *mut SpaceBitmap<8> {
        &self.live_bitmap as *const _ as *mut _
    }
}

pub struct MallocSpace {
    pub space: ContinuousMemMapAllocSpace,
    pub growth_limit: usize,
    pub can_move_objects: bool,
    pub starting_size: usize,
    pub initial_size: usize,
}

deref_impl!(MallocSpace; ContinuousMemMapAllocSpace where space);

impl MallocSpace {
    #[inline]
    pub fn disable_moving_objects(&mut self) {
        self.can_move_objects = false;
    }

    #[inline]
    pub fn can_move_objects(&self) -> bool {
        self.can_move_objects
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.growth_limit
    }

    #[inline]
    pub fn non_growth_limit_capacity(&self) -> usize {
        self.space.mem_map.size()
    }

    #[inline]
    pub fn clear_growth_limit(&mut self) {
        self.growth_limit = self.non_growth_limit_capacity();
    }

    #[inline]
    pub fn new(
        name: &str,
        mem_map: Mmap,
        begin: *mut u8,
        end: *mut u8,
        limit: *mut u8,
        growth_limit: usize,
        create_bitmaps: bool,
        can_move_objects: bool,
        starting_size: usize,
        initial_size: usize,
    ) -> Self {
        let mut this = Self {
            space: ContinuousMemMapAllocSpace::new(name, mem_map, begin, end, limit),
            growth_limit,
            can_move_objects,
            starting_size,
            initial_size,
        };
        if create_bitmaps {
            this.allocate_bitmaps();
        }
        this
    }

    pub fn create_mem_map(
        starting_size: usize,
        initial_size: &mut usize,
        growth_limit: &mut usize,
        capacity: &mut usize,
    ) -> Mmap {
        if starting_size > *initial_size {
            *initial_size = starting_size;
        }

        if *initial_size > *growth_limit {
            panic!(
                "failed to create alloc space {} > {}",
                initial_size, growth_limit
            );
        }

        if *growth_limit > *capacity {
            panic!("failed to create alloc space");
        }

        *growth_limit = round_up(*growth_limit as _, PAGE_SIZE as _) as _;
        *capacity = round_up(*capacity as _, PAGE_SIZE as _) as _;
        let mem_map = Mmap::new(*capacity, 8);

        mem_map
    }

    pub fn set_growth_limit(&mut self, mut growth_limit: usize) {
        growth_limit = round_up(growth_limit as _, PAGE_SIZE as _) as _;
        self.growth_limit = growth_limit;
        if self.size() > self.growth_limit {
            unsafe {
                self.set_end(self.begin.add(self.growth_limit));
            }
        }
    }

    pub unsafe fn morecore(&mut self, increment: isize) -> *mut u8 {
        let original_end = self.end();
        if increment != 0 {
            let new_end = original_end.offset(increment);
            if increment > 0 {
                self.get_mem_map().commit(original_end, increment as _);
            } else {
                let size = (-increment) as usize;
                self.get_mem_map().decommit(new_end, size);
            }
            self.set_end(new_end);
        }
        original_end
    }

    pub fn clamp_growth_limit(&mut self) {
        let new_capacity = self.capacity();
        self.live_bitmap.set_heap_size(new_capacity);
        self.mark_bitmap.set_heap_size(new_capacity);
        if !self.temp_bitmap.is_null() {
            self.temp_bitmap.set_heap_size(new_capacity);
        }
        self.limit = (self.begin() as usize + new_capacity) as _;
    }
}
