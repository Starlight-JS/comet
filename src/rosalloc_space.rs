use std::{
    marker::PhantomData,
    ptr::{null_mut, NonNull},
};

use rosalloc::{
    dedicated_full_run,
    defs::{
        PageReleaseMode, DEFAULT_PAGE_RELEASE_THRESHOLD, NUM_THREAD_LOCAL_SIZE_BRACKETS, PAGE_SIZE,
    },
    Rosalloc, Run,
};

use crate::{
    api::{vtable_of, Gc, HeapObjectHeader},
    gc_base::{GcBase, TLAB},
    mutator::MutatorRef,
    small_type_id,
    space::MallocSpace,
    utils::{align_usize, mmap::Mmap},
};
use parking_lot::{lock_api::RawMutex, RawMutex as Lock};
pub struct RosAllocSpace {
    space: MallocSpace,
    rosalloc: *mut Rosalloc,
    #[allow(dead_code)]
    low_memory_mode: bool,
    lock: Lock,
}

deref_impl!(RosAllocSpace;MallocSpace where space);

impl RosAllocSpace {
    pub fn rosalloc(&self) -> *mut Rosalloc {
        self.rosalloc
    }
    pub fn alloc_with_growth<H: GcBase<TLAB = RosAllocTLAB>>(
        &mut self,
        mutator: &mut MutatorRef<H>,
        num_bytes: usize,
        bytes_allocated: &mut usize,
        usable_size: &mut usize,
        bytes_tl_bulk_allocated: &mut usize,
    ) -> *mut u8 {
        let result;
        unsafe {
            self.lock.lock();
            let max_allowed = self.capacity();
            (*self.rosalloc).set_footprint_limit(max_allowed);
            result = self.alloc_common::<H, true>(
                mutator,
                num_bytes,
                bytes_allocated,
                usable_size,
                bytes_tl_bulk_allocated,
            );
            let footprint = (*self.rosalloc).footprint();
            (*self.rosalloc).set_footprint_limit(footprint);
            self.lock.unlock();
        }

        result
    }

    #[inline]
    pub unsafe fn alloc_common<H: GcBase<TLAB = RosAllocTLAB>, const THREAD_SAFE: bool>(
        &mut self,
        mutator: &mut MutatorRef<H>,
        num_bytes: usize,
        bytes_allocated: &mut usize,
        usable_size: &mut usize,
        bytes_tl_bulk_allocated: &mut usize,
    ) -> *mut u8 {
        let result = (*self.rosalloc).alloc::<THREAD_SAFE>(
            &mut mutator.tlab.runs,
            num_bytes,
            bytes_allocated,
            usable_size,
            bytes_tl_bulk_allocated,
        );
        result
    }
    pub fn create(
        name: &str,
        mut initial_size: usize,
        mut growth_limit: usize,
        mut capacity: usize,
        low_memory_mode: bool,
        can_move_objects: bool,
    ) -> *mut Self {
        let starting_size = PAGE_SIZE;
        let mem_map = MallocSpace::create_mem_map(
            starting_size,
            &mut initial_size,
            &mut growth_limit,
            &mut capacity,
        );
        let space = Self::create_from_mem_map(
            mem_map,
            name,
            starting_size,
            initial_size,
            growth_limit,
            capacity,
            low_memory_mode,
            can_move_objects,
        );
        space
    }
    pub fn create_from_mem_map(
        mem_map: Mmap,
        name: &str,
        starting_size: usize,
        initial_size: usize,
        growth_limit: usize,
        capacity: usize,
        low_memory_mode: bool,
        can_move_objects: bool,
    ) -> *mut Self {
        unsafe {
            let rosalloc = Self::create_rosalloc(
                mem_map.start(),
                starting_size,
                initial_size,
                capacity,
                low_memory_mode,
            );
            let end = mem_map.start().add(starting_size);
            let begin = mem_map.start();
            let this = Box::into_raw(Box::new(Self::new(
                mem_map,
                initial_size,
                name,
                rosalloc,
                begin,
                end,
                begin.add(capacity),
                growth_limit,
                can_move_objects,
                starting_size,
                low_memory_mode,
            )));

            (*(*this).rosalloc).set_morecore(morecore, this.cast());

            this
        }
    }

    pub fn new(
        mem_map: Mmap,
        initial_size: usize,
        name: &str,
        rosalloc: *mut Rosalloc,
        begin: *mut u8,
        end: *mut u8,
        limit: *mut u8,
        growth_limit: usize,
        can_move_objects: bool,
        starting_size: usize,
        low_memory_mode: bool,
    ) -> Self {
        Self {
            space: MallocSpace::new(
                name,
                mem_map,
                begin,
                end,
                limit,
                growth_limit,
                true,
                can_move_objects,
                starting_size,
                initial_size,
            ),
            rosalloc,
            low_memory_mode,
            lock: Lock::INIT,
        }
    }
    pub fn create_rosalloc(
        begin: *mut u8,
        morecore_start: usize,
        initial_size: usize,
        maximum_size: usize,
        low_memory_mode: bool,
    ) -> *mut Rosalloc {
        unsafe {
            let rosalloc = Rosalloc::new(
                begin,
                morecore_start,
                maximum_size,
                if low_memory_mode {
                    PageReleaseMode::All
                } else {
                    PageReleaseMode::SizeAndEnd
                },
                DEFAULT_PAGE_RELEASE_THRESHOLD,
            );
            (*rosalloc).set_footprint_limit(initial_size);
            rosalloc
        }
    }

    pub fn sweep_callback(&mut self, ptrs: &[*mut u8], swap_bitmaps: bool) -> usize {
        if !swap_bitmaps {
            let bitmap = self.get_live_bitmap();

            unsafe {
                for ptr in ptrs.iter() {
                    (*bitmap).clear(*ptr);
                }
            }
        }
        unsafe { (*self.rosalloc).bulk_free(ptrs) }
    }
}

/// RosAlloc space thread local allocation buffer. Must not be used directly by mutator but should be used from [GcBase::allocate_inline]. This is due
/// to possibility of refilling runs at allocation time.
pub struct RosAllocTLAB {
    pub rosalloc: *mut RosAllocSpace,
    pub runs: [*mut Run; NUM_THREAD_LOCAL_SIZE_BRACKETS],
}

impl<H: GcBase<TLAB = Self>> TLAB<H> for RosAllocTLAB {
    fn reset(&mut self) {
        unsafe {
            let rosalloc = &mut *((*self.rosalloc).rosalloc);
            rosalloc.revoke_thread_local_runs(&mut self.runs);
        }
    }
    #[inline]
    fn can_thread_local_allocate(&self, size: usize) -> bool {
        Rosalloc::is_size_for_thread_local(size)
    }

    fn create(heap: std::sync::Arc<std::cell::UnsafeCell<H>>) -> Self {
        unsafe {
            let mut tlab = Self {
                rosalloc: null_mut(),
                runs: [dedicated_full_run(); NUM_THREAD_LOCAL_SIZE_BRACKETS],
            };

            (*heap.get()).init_tlab(&mut tlab);
            tlab
        }
    }
    #[inline]
    fn allocate<T: crate::api::Collectable + 'static>(
        &mut self,
        value: T,
    ) -> Result<crate::api::Gc<T>, T> {
        unsafe {
            let size = align_usize(value.allocation_size(), 8);
            //let rosalloc = &mut *((*self.rosalloc).rosalloc);
            let (idx, _bracket_size) = Rosalloc::size_to_index_and_bracket_size(size);
            let thread_local_run = &mut *self.runs[idx];
            let slot_addr = (*thread_local_run).alloc_slot();
            if slot_addr.is_null() {
                return Err(value);
            }

            let header = slot_addr.cast::<HeapObjectHeader>();
            header.write(HeapObjectHeader {
                type_id: small_type_id::<T>(),
                padding: 0,
                padding2: 0,
                value: 0,
            });
            (*header).set_vtable(vtable_of::<T>());
            (*header).set_size(size);
            Ok(Gc {
                base: NonNull::new_unchecked(header),
                marker: PhantomData,
            })
        }
    }

    /// Always returns false. Rosalloc space automatically refils run if possible at allocation time
    fn refill(&mut self, _mutator: &MutatorRef<H>, _size: usize) -> bool {
        false
    }
}

extern "C" fn morecore(_rosalloc: *mut Rosalloc, increment: isize, data: *mut u8) {
    unsafe {
        let space = &mut *data.cast::<RosAllocSpace>();

        space.morecore(increment);
    }
}
