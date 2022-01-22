//! # SemiSpace
//!
//! Simple semi-space garbage collector that separates heap into two spaces: "to space" and "from space". During mutator time
//! all allocations go to "to space" and when it is full they are swapped and all objects are copied from "from space" to "to space".
//! If there is no enough memory to copy object process will abort with OOM error.

use std::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem::size_of,
    ptr::{null_mut, NonNull},
    sync::Arc,
};

use crate::{
    api::{vtable_of, Collectable, Gc, HeapObjectHeader, Trace, Visitor, Weak},
    bump_pointer_space::BumpPointerSpace,
    gc_base::{AllocationSpace, GcBase, MarkingConstraint, MarkingConstraintRuns, NoReadBarrier},
    large_space::{LargeObjectSpace, PreciseAllocation},
    make_small_type_id,
    mutator::{oom_abort, JoinData, Mutator, MutatorRef, ThreadState},
    safepoint::{GlobalSafepoint, SafepointScope},
    small_type_id,
    tlab::{InlineAllocationHelpersForSimpleTLAB, SimpleTLAB},
    utils::align_usize,
};

use atomic::Ordering;
use im::Vector;
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
    weak_refs: Vec<Weak<dyn Collectable, Self>>,
    constraints: Vec<Box<dyn MarkingConstraint>>,
    finalize_list: Vector<*mut HeapObjectHeader>,
    finalize_lock: Lock,
}

pub fn instantiate_semispace(semispace_size: usize) -> MutatorRef<SemiSpace> {
    let heap = Arc::new(UnsafeCell::new(SemiSpace {
        global_heap_lock: Lock::INIT,
        large_space_lock: Lock::INIT,
        safepoint: GlobalSafepoint::new(),
        large_space: LargeObjectSpace::new(),
        mutators: Vec::new(),
        mark_stack: Vec::new(),
        finalize_list: Vector::new(),
        finalize_lock: Lock::INIT,
        constraints: vec![],
        from_space: BumpPointerSpace::new(semispace_size),
        to_space: BumpPointerSpace::new(semispace_size),
        weak_refs: vec![],
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
    unsafe fn after_mark_constraints(&mut self) {
        let this = self as *mut Self;
        (*this).constraints.retain_mut(|constraint| {
            if constraint.is_over() {
                false
            } else {
                if constraint.runs_at() == MarkingConstraintRuns::AfterMark {
                    constraint.run(self);
                }
                true
            }
        });
    }
    unsafe fn before_mark_constraints(&mut self) {
        let this = self as *mut Self;
        (*this).constraints.retain_mut(|constraint| {
            if constraint.is_over() {
                false
            } else {
                if constraint.runs_at() == MarkingConstraintRuns::BeforeMark {
                    constraint.run(self);
                }
                true
            }
        });
    }
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
    type ReadBarrier = NoReadBarrier;
    type InlineAllocationHelpers = InlineAllocationHelpersForSimpleTLAB;
    fn inline_allocation_helpers(&self) -> Self::InlineAllocationHelpers {
        InlineAllocationHelpersForSimpleTLAB
    }
    fn add_constraint<T: MarkingConstraint + 'static>(&mut self, constraint: T) {
        self.global_lock();
        self.constraints.push(Box::new(constraint));
        self.global_unlock();
    }
    fn post_alloc<T: Collectable + Sized + 'static>(&mut self, value: Gc<T, Self>) {
        if std::mem::needs_drop::<T>() {
            unsafe {
                self.finalize_lock.lock();
                self.finalize_list.push_front(value.base.as_ptr());
                self.finalize_lock.unlock();
            }
        }
    }
    fn allocate_weak<T: Collectable + ?Sized>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        value: Gc<T, Self>,
    ) -> Weak<T, Self> {
        let weak_ref = unsafe { Weak::create(mutator, value) };
        self.global_heap_lock.lock();
        self.weak_refs.push(weak_ref.to_dyn());
        unsafe {
            self.global_heap_lock.unlock();
        }
        weak_ref
    }
    fn alloc_tlab_area(&mut self, _mutator: &MutatorRef<Self>, _size: usize) -> *mut u8 {
        let memory = self.to_space.bump_alloc(32 * 1024);
        memory
    }
    #[inline]
    fn allocate_raw(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        size: usize,
        type_id: std::any::TypeId,
        vtable: usize,
    ) -> *mut HeapObjectHeader {
        let size = align_usize(size + size_of::<HeapObjectHeader>(), 8);
        let mut memory = self.to_space.bump_alloc(size);
        if memory.is_null() {
            self.collect(mutator, &mut []);
            memory = self.to_space.bump_alloc(size);
            if memory.is_null() {
                oom_abort();
            }
        }

        unsafe {
            let hdr = memory.cast::<HeapObjectHeader>();
            (*hdr).set_vtable(vtable);
            (*hdr).set_size(size);
            (*hdr).type_id = make_small_type_id(type_id);
            let val: Gc<(), Self> = Gc {
                base: NonNull::new_unchecked(hdr),
                marker: Default::default(),
            };
            self.post_alloc(val);
            hdr
        }
    }
    #[inline]
    fn alloc_inline<T: crate::api::Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        mut value: T,
        _: AllocationSpace,
    ) -> crate::api::Gc<T, Self> {
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
            (*hdr).set_metadata(vtable_of::<T>());
            (*hdr).set_size(size);
            (*hdr).type_id = small_type_id::<T>();
            ((*hdr).data() as *mut T).write(value);

            let val = Gc {
                base: NonNull::new_unchecked(hdr),
                marker: Default::default(),
            };
            self.post_alloc(val);
            val
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
    ) -> crate::api::Gc<T, Self> {
        unsafe {
            let size = value.allocation_size() + size_of::<HeapObjectHeader>();
            self.large_space_lock.lock();
            let object = self.large_space.allocate(size);
            (*object).set_metadata(vtable_of::<T>());
            (*object).type_id = small_type_id::<T>();
            let gc = Gc {
                base: NonNull::new_unchecked(object),
                marker: PhantomData,
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
                unsafe {
                    self.before_mark_constraints();
                }
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
                unsafe {
                    self.after_mark_constraints();
                }
                self.finalize_list.retain(|x| unsafe {
                    let object = *x;
                    if (*object).is_forwarded() {
                        return true;
                    }
                    (*object).get_dyn().finalize();
                    false
                });
                self.weak_refs.retain_mut(|object| unsafe {
                    let header = object.base();
                    if (*header).is_forwarded() {
                        let header = (*header).vtable() as *mut HeapObjectHeader;
                        object.set_base(header);
                        object.after_mark(|object| {
                            if (*object).is_forwarded() {
                                (*object).vtable() as _
                            } else {
                                null_mut()
                            }
                        });

                        true
                    } else {
                        false
                    }
                });
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
