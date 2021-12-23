use std::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem::size_of,
    ptr::{null_mut, NonNull},
    sync::Arc,
};

use crate::{
    api::{vtable_of, Gc, HeapObjectHeader},
    gc_base::{GcBase, TLAB},
    mutator::MutatorRef,
    small_type_id,
    utils::align_usize,
};

/// Simple thread local allocation buffer implementation. This TLAB uses 32KB buffer requested from GC and allocates objects up to 8KB in size in it.
pub struct SimpleTLAB<H: GcBase> {
    pub heap: Arc<UnsafeCell<H>>,
    pub tlab_start: *mut u8,
    pub tlab_cursor: *mut u8,
    pub tlab_end: *mut u8,
}

impl<H: GcBase> SimpleTLAB<H> {}

impl<H: GcBase<TLAB = Self>> TLAB<H> for SimpleTLAB<H> {
    fn can_thread_local_allocate(&self, size: usize) -> bool {
        size <= 8 * 1024
    }
    #[inline]
    fn allocate<T: crate::api::Collectable + 'static>(
        &mut self,
        value: T,
    ) -> Result<crate::api::Gc<T>, T> {
        if self.tlab_cursor.is_null() {
            return Err(value);
        }
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        unsafe {
            let result = self.tlab_cursor;
            let new_cursor = result.add(size);
            if new_cursor > self.tlab_end {
                return Err(value);
            }
            self.tlab_cursor = new_cursor;
            let header = result.cast::<HeapObjectHeader>();
            header.write(HeapObjectHeader {
                type_id: small_type_id::<T>(),
                padding: 0,
                padding2: 0,
                value: 0,
            });
            (*header).set_vtable(vtable_of::<T>());
            (*header).set_size(size);
            ((*header).data() as *mut T).write(value);
            let h = &mut *self.heap.get();
            let gc = Gc {
                base: NonNull::new_unchecked(header),
                marker: PhantomData,
            };
            h.post_alloc(gc);
            Ok(gc)
        }
    }

    fn refill(&mut self, mutator: &MutatorRef<H>, _size: usize) -> bool {
        unsafe {
            let h = &mut *self.heap.get();
            let tlab = h.alloc_tlab_area(mutator, 32 * 1024);
            if tlab.is_null() {
                return false;
            }
            self.tlab_start = tlab;
            self.tlab_end = tlab.add(32 * 1024);
            self.tlab_cursor = tlab;
            true
        }
    }

    fn reset(&mut self) {
        self.tlab_cursor = null_mut();
        self.tlab_end = null_mut();
        self.tlab_start = null_mut();
    }
    fn create(heap: Arc<UnsafeCell<H>>) -> Self {
        Self {
            heap,
            tlab_start: null_mut(),
            tlab_cursor: null_mut(),
            tlab_end: null_mut(),
        }
    }
}
