use std::{cell::UnsafeCell, mem::size_of, ptr::null_mut, sync::Arc};

use crate::{
    alloc::array::Array,
    api::{vtable_of, Collectable, Gc, HeapObjectHeader, Trace},
    mutator::{Mutator, MutatorRef},
    rosalloc_space::RosAllocSpace,
    safepoint::GlobalSafepoint,
};

pub trait GcBase: Sized {
    const LARGE_ALLOCATION_SIZE: usize = 16 * 1024;
    const SUPPORTS_TLAB: bool = false;

    type TLAB: TLAB<Self>;

    fn get_rosalloc_space(&self) -> *mut RosAllocSpace {
        null_mut()
    }

    fn attach_current_thread(&mut self, mutator: *mut Mutator<Self>);
    fn detach_current_thread(&mut self, mutator: *mut Mutator<Self>);

    fn safepoint(&self) -> &GlobalSafepoint;

    fn global_lock(&self);
    fn global_unlock(&self);
    fn mutators(&self) -> &[*mut Mutator<Self>];

    /// allocates 32 KB TLAB area
    fn alloc_tlab_area(&mut self, mutator: &MutatorRef<Self>, size: usize) -> *mut u8;

    /// Inline allocation function. Might be used instead of TLAB or when allocation size overflows tlab large allocation size.
    ///
    ///
    /// How this function should be implemented ideally:
    /// - Lockless in fast-path
    /// - Atomic bump-pointer/thread-local bump pointer or atomic freelist/thread-local freelist.
    ///
    /// Bump pointer might be used in Immix or SemiSpace GCs. While freelists might be used in case of Mark&Sweep GC.
    fn alloc_inline<T: Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        value: T,
    ) -> Gc<T>;

    /// Post allocation operation e.g set mark in bitmap that this object was allocated.
    ///
    /// Restrictions for this function:
    /// - Must not acquire any mutex locks when `needs_drop::<T>()` returns false
    /// - Must not do CPU heavy operations
    /// - Must put `value` to finalizer list if `needs_drop::<T>()` returns true
    #[inline(always)]
    fn post_alloc<T: Collectable + Sized + 'static>(&mut self, value: Gc<T>) {
        let _ = value;
    }

    fn allocate_large<T: Collectable + Sized + 'static>(
        &mut self,
        mutator: &MutatorRef<Self>,
        value: T,
    ) -> Gc<T>;

    fn minor_collection(&mut self, mutator: &MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        self.full_collection(mutator, keep);
    }
    fn full_collection(&mut self, mutator: &MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        self.collect(mutator, keep);
    }

    fn collect(&mut self, mutator: &MutatorRef<Self>, keep: &mut [&mut dyn Trace]);

    fn write_barrier(&mut self, object: Gc<dyn Collectable>) {
        let _ = object;
    }

    fn init_tlab(&mut self, tlab: &mut Self::TLAB) {
        let _ = tlab;
    }
}

pub trait TLAB<H: GcBase<TLAB = Self>> {
    fn can_thread_local_allocate(&self, size: usize) -> bool;
    fn allocate<T: Collectable + 'static>(&mut self, value: T) -> Result<Gc<T>, T>;
    fn refill(&mut self, mutator: &MutatorRef<H>, alloc_size: usize) -> bool;
    fn reset(&mut self);
    fn create(heap: Arc<UnsafeCell<H>>) -> Self;
}

pub unsafe fn fill_region(start: *mut u8, end: *mut u8) {
    if start == end {
        // nothing to do
    } else if end.offset_from(start) == size_of::<usize>() as _ {
        *start.cast::<usize>() = 0;
    } else if end.offset_from(start) == size_of::<HeapObjectHeader>() as _ {
        let header = start.cast::<HeapObjectHeader>();
        (*header).set_vtable(vtable_of::<()>());
        (*header).set_size(size_of::<HeapObjectHeader>());
    } else {
        let array_header = start.cast::<HeapObjectHeader>();
        (*array_header).set_vtable(vtable_of::<Array<i32>>());
        let array = (*array_header).data().cast::<Array<i32>>() as *mut Array<i32>;
        (*array).is_inited = false;
        (*array).length = end.offset_from((*array).data().cast::<u8>()) as u32 / 4;
        (*array_header).set_size((*array).allocation_size());
    }
}
