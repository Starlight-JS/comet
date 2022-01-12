use std::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem::size_of,
    ptr::{null_mut, NonNull},
    sync::Arc,
};

use crate::{
    api::{vtable_of, Collectable, Gc, HeapObjectHeader},
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
    ) -> Result<crate::api::Gc<T, H>, T> {
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

/// Inline allocation helpers for [SimpleTLAB](SimpleTLAB). These helpers are the easiest one to use: simply load fields from offsets provided
/// by these helpers and bump allocate into TLAB.
///
/// ## Example
/// **NOTE**: Pseudocode
///
/// ```rust
/// fn emit_alloc_object<H: GcBase<TLAB=SimpleTLAB<H>>(&mut self,size: usize,mutator: &MutatorRef<H>) {
///     let helpers = mutator.inline_helpers();
///     let cursor_offset = helpers.tlab_cursor_offset(mutator);
///     let end_offset = helpers.tlab_end_offset(mutator);
///     
///     self.load_offset(MUTATOR_REG, cursor_offset, TMP1);
///     self.load_offset(MUTATOR_REG, end_offset, TMP2);
///     self.iadd(TMP3, TMP1, size + size_of::<HeapObjectHeader>()); // new cursor
///     self.branch_if_greater_than(TMP3,TMP2, <slowpath>); // if new cursor > TLAB end then jump to slowpath, slowpath might just be call to runtime function which does allocation.
///     self.store_offset(MUTATOR_REG, cursor_offset, TMP3); // update TLAB cursor
///     self.store_offset(TMP1, 0, helpers.alloc_heap_object_header_for::<Object>()) // store heap object header at 0 offset. Memory that is usable is at TMP1 + size_of::<HeapObjectHeader>()
///     // done! Memory is now allocated in TMP1 register, usable memory starts from TMP1 + size_of::<HeapObjectHeader>()
/// }
///
/// ```
///
pub struct InlineAllocationHelpersForSimpleTLAB;

impl InlineAllocationHelpersForSimpleTLAB {
    pub fn tlab_end_offset<Heap: GcBase<TLAB = SimpleTLAB<Heap>>>(
        &self,
        mutator: &MutatorRef<Heap>,
    ) -> usize {
        unsafe {
            let start = mutator.ptr() as usize;
            let end = &mutator.tlab().tlab_end as *const _ as usize;
            end - start
        }
    }

    pub fn tlab_start_offset<Heap: GcBase<TLAB = SimpleTLAB<Heap>>>(
        &self,
        mutator: &MutatorRef<Heap>,
    ) -> usize {
        unsafe {
            let start = mutator.ptr() as usize;
            let end = &mutator.tlab().tlab_start as *const _ as usize;
            end - start
        }
    }

    pub fn tlab_cursor_offset<Heap: GcBase<TLAB = SimpleTLAB<Heap>>>(
        &self,
        mutator: &MutatorRef<Heap>,
    ) -> usize {
        unsafe {
            let start = mutator.ptr() as usize;
            let end = &mutator.tlab().tlab_cursor as *const _ as usize;
            end - start
        }
    }

    pub fn alloc_heap_object_header_for<T: Collectable + 'static>(
        &self,
        size: usize,
    ) -> HeapObjectHeader {
        let mut hdr = HeapObjectHeader {
            value: 0,
            type_id: 0,
            padding: 0,
            padding2: 0,
        };
        hdr.set_vtable(vtable_of::<T>());
        assert!(
            size <= 64 * 1024,
            "allocation size too large to be inlineable"
        );
        hdr.set_size(size);
        hdr.type_id = small_type_id::<T>();
        hdr
    }
}
