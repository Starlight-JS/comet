//! TODO

use std::{cell::UnsafeCell, marker::PhantomData, mem::size_of, ptr::NonNull, sync::Arc};

use crate::{
    api::{vtable_of, Collectable, Gc, HeapObjectHeader, Trace, Visitor},
    bump_pointer_space::BumpPointerSpace,
    gc_base::{fill_region, GcBase},
    large_space::{LargeObjectSpace, PreciseAllocation},
    mutator::{oom_abort, JoinData, Mutator, MutatorRef, ThreadState},
    safepoint::{GlobalSafepoint, SafepointScope},
    small_type_id,
    tlab::SimpleTLAB,
    utils::align_usize,
};

use atomic::Ordering;
use parking_lot::{lock_api::RawMutex, RawMutex as Lock};

pub struct MiniMark {
    pub(crate) global_heap_lock: Lock,
    pub(crate) large_space_lock: Lock,
    pub(crate) large_space: LargeObjectSpace,
    pub(crate) nursery_space: BumpPointerSpace,
    pub(crate) mutators: Vec<*mut Mutator<Self>>,
    pub(crate) safepoint: GlobalSafepoint,
    pub(crate) mark_stack: Vec<*mut HeapObjectHeader>,
}

impl GcBase for MiniMark {
    type TLAB = SimpleTLAB<Self>;
    fn alloc_inline<T: Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        value: T,
    ) -> Gc<T> {
        todo!()
    }
    fn alloc_tlab_area(&mut self, mutator: &MutatorRef<Self>, _size: usize) -> *mut u8 {
        todo!()
    }
    fn safepoint(&self) -> &GlobalSafepoint {
        &self.safepoint
    }

    fn attach_current_thread(&mut self, mutator: *mut Mutator<Self>) {
        self.global_heap_lock.lock();
        self.safepoint.n_mutators.fetch_add(1, Ordering::Relaxed);
        self.mutators.push(mutator);
        unsafe { self.global_heap_lock.unlock() };
    }

    fn detach_current_thread(&mut self, mutator: *mut Mutator<Self>) {
        self.global_heap_lock.lock();

        let mut detached = false;
        self.mutators.retain(|x| {
            let x = *x;
            let y = mutator;
            if x == y {
                detached = true;
                false
            } else {
                true
            }
        });
        assert!(detached, "mutator must be detached");
        unsafe {
            self.global_heap_lock.unlock();
        }
    }

    fn global_lock(&self) {
        self.global_heap_lock.lock();
    }
    fn global_unlock(&self) {
        unsafe {
            debug_assert!(self.global_heap_lock.is_locked());
            self.global_heap_lock.unlock();
        }
    }

    fn mutators(&self) -> &[*mut Mutator<Self>] {
        assert!(self.global_heap_lock.is_locked());
        &self.mutators
    }

    fn allocate_large<T: Collectable + Sized + 'static>(
        &mut self,
        _mutator: &MutatorRef<Self>,
        value: T,
    ) -> crate::api::Gc<T> {
        unsafe {
            let size = value.allocation_size() + size_of::<HeapObjectHeader>();
            self.large_space_lock.lock();
            let object = self.large_space.allocate(size);
            (*object).set_vtable(vtable_of::<T>());
            (*object).type_id = small_type_id::<T>();
            let gc = Gc {
                base: NonNull::new_unchecked(object),
                marker: PhantomData::<T>,
            };
            ((*object).data() as *mut T).write(value);
            self.large_space_lock.unlock();
            self.post_alloc(gc);
            gc
        }
    }

    fn collect(&mut self, mutator: &MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {}
}
