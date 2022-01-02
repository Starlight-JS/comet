use std::{cell::UnsafeCell, marker::PhantomData, mem::size_of, ptr::NonNull, sync::Arc};

use crate::{
    api::{vtable_of, Collectable, Gc, HeapObjectHeader, Trace, Visitor},
    bump_pointer_space::BumpPointerSpace,
    gc_base::{AllocationSpace, GcBase},
    large_space::{LargeObjectSpace, PreciseAllocation},
    mutator::{oom_abort, JoinData, Mutator, MutatorRef, ThreadState},
    safepoint::{GlobalSafepoint, SafepointScope},
    small_type_id,
    tlab::SimpleTLAB,
    utils::align_usize,
};

use atomic::Ordering;
use parking_lot::{lock_api::RawMutex, RawMutex as Lock};

pub struct SemiSpace {
    pub(crate) global_heap_lock: Lock,
    pub(crate) large_space_lock: Lock,
    pub(crate) large_space: LargeObjectSpace,
    pub(crate) from_space: BumpPointerSpace,
    pub(crate) to_space: BumpPointerSpace,
    pub(crate) mutators: Vec<*mut Mutator<Self>>,
    pub(crate) safepoint: GlobalSafepoint,
    pub(crate) mark_stack: Vec<*mut HeapObjectHeader>,
}

pub fn instantiate_semispace(semispace_size: usize) -> MutatorRef<SemiSpace> {
    let heap = Arc::new(UnsafeCell::new(SemiSpace {
        global_heap_lock: Lock::INIT,
        large_space_lock: Lock::INIT,
        safepoint: GlobalSafepoint::new(),
        large_space: LargeObjectSpace::new(),
        mutators: Vec::new(),
        mark_stack: Vec::new(),

        from_space: BumpPointerSpace::new(semispace_size),
        to_space: BumpPointerSpace::new(semispace_size),
    }));

    let href = unsafe { &mut *heap.get() };
    href.to_space.commit();
    let join_data = JoinData::new();
    let mut mutator = MutatorRef::new(Mutator::new(
        heap.clone(),
        &href.safepoint,
        join_data.internal.clone(),
    ));
    href.mutators.push(&mut *mutator);
    href.safepoint
        .n_mutators
        .fetch_add(1, atomic::Ordering::Relaxed);
    mutator.state_set(ThreadState::Safe, ThreadState::Unsafe);
    mutator
}

impl SemiSpace {
    fn trace(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        unsafe {
            let object = root.as_ptr();
            if !self.from_space.contains(object.cast()) && (*object).is_precise() {
                if !(*PreciseAllocation::from_cell(object)).test_and_set_marked() {
                    self.mark_stack.push(object);
                }
            } else if (*object).is_forwarded() {
                *root = NonNull::new_unchecked((*object).vtable() as _);
            } else {
                let size = (*object).size();
                let mem = self.to_space.bump_alloc(size);
                core::ptr::copy_nonoverlapping(object.cast::<u8>(), mem, size);
                (*object).set_forwarded(mem as _);
                *root = NonNull::new_unchecked(mem as _);

                self.mark_stack.push(mem.cast());
            }
        }
    }
}

impl GcBase for SemiSpace {
    const SUPPORTS_TLAB: bool = true;
    type TLAB = SimpleTLAB<Self>;

    fn alloc_tlab_area(&mut self, _mutator: &MutatorRef<Self>, _size: usize) -> *mut u8 {
        let memory = self.to_space.bump_alloc(32 * 1024);
        memory
    }
    fn alloc_inline<T: crate::api::Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        mut value: T,
        _: AllocationSpace,
    ) -> crate::api::Gc<T> {
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        let mut memory = self.to_space.bump_alloc(size);
        if memory.is_null() {
            self.collect(mutator, &mut [&mut value]);
            memory = self.to_space.bump_alloc(size);
            if memory.is_null() {
                oom_abort();
            }
        }

        unsafe {
            let hdr = memory.cast::<HeapObjectHeader>();
            (*hdr).set_vtable(vtable_of::<T>());
            (*hdr).set_size(size);
            ((*hdr).data() as *mut T).write(value);
            Gc {
                base: NonNull::new_unchecked(hdr),
                marker: Default::default(),
            }
        }
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
        self.safepoint.n_mutators.fetch_sub(1, Ordering::Relaxed);
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
        _mutator: &mut MutatorRef<Self>,
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
    fn collect(&mut self, mutator: &mut MutatorRef<Self>, mut keep: &mut [&mut dyn Trace]) {
        match SafepointScope::new(mutator.clone()) {
            Some(safepoint) => {
                self.global_heap_lock.lock();
                self.large_space_lock.lock();

                std::mem::swap(&mut self.from_space, &mut self.to_space);
                //self.to_space.commit();
                self.large_space.prepare_for_marking(false);
                for i in 0..self.mutators.len() {
                    unsafe {
                        let mutator = self.mutators[i];
                        //fill_region((*mutator).tlab.cursor, (*mutator).tlab_end);

                        //  (*mutator).reset_tlab();
                        (*mutator).reset_tlab();
                        (*mutator).shadow_stack().walk(|object| {
                            object.trace(self);
                        });
                    }
                }
                keep.trace(self);

                while let Some(object) = self.mark_stack.pop() {
                    unsafe {
                        (*object).get_dyn().trace(self);
                    }
                }

                self.large_space.sweep();
                self.large_space.prepare_for_allocation(false);
                self.from_space.reset();
                //self.from_space.decommit();
                drop(safepoint);
                unsafe {
                    self.global_heap_lock.unlock();
                    self.large_space_lock.unlock();
                }
            }
            None => return,
        }
    }
}

impl Visitor for SemiSpace {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        self.trace(root);
    }
}
