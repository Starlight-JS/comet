#![allow(dead_code)]
use crate::{
    api::{vtable_of, Collectable, Gc, HeapObjectHeader, Trace, GC_BLACK, GC_GREY},
    bump_pointer_space::BumpPointerSpace,
    gc_base::{AllocationSpace, GcBase, TLAB},
    mutator::{oom_abort, Mutator, MutatorRef},
    safepoint::GlobalSafepoint,
    small_type_id,
    utils::align_usize,
};
use atomic::Ordering;
use flume::{Receiver, Sender};
use parking_lot::{lock_api::RawMutex, RawMutex as Lock};
use std::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem::size_of,
    ptr::{null_mut, NonNull},
    sync::Arc,
};

pub const REGION_SIZE: usize = 4 * 1024 * 1024;
pub const TLAB_WB_BUFFER_SIZE: usize = 48;

/// Concurrent semispace collector.
pub struct ConcSemispace {
    write_barrier_buffer: Vec<*mut HeapObjectHeader>,
    write_barrier_lock: Lock,

    gc_request_snd: Sender<()>,
    gc_request_recv: Receiver<()>,
    gc_finish_recv: Receiver<()>,
    gc_finish_snd: Sender<()>,

    to_space: BumpPointerSpace,
    from_space: BumpPointerSpace,
    pub(crate) mutators: Vec<*mut Mutator<Self>>,
    pub(crate) safepoint: GlobalSafepoint,
    pub(crate) mark_stack: Vec<*mut HeapObjectHeader>,
    global_heap_lock: Lock,
}

impl ConcSemispace {}

unsafe fn collector_routine(heap: *mut ConcSemispace) {
    let heap = &mut *heap;

    loop {
        match heap.gc_request_recv.recv() {
            Ok(()) => {}
            Err(_) => break,
        }
    }
}

pub struct ConcSemispaceTLAB {
    heap: Arc<UnsafeCell<ConcSemispace>>,
    pub tlab_start: *mut u8,
    pub tlab_cursor: *mut u8,
    pub tlab_end: *mut u8,
    buffer: Box<[*mut HeapObjectHeader; TLAB_WB_BUFFER_SIZE]>,
    buffer_cursor: *mut *mut HeapObjectHeader,
    buffer_end: *mut *mut HeapObjectHeader,
    is_conc_mark: bool,
}

impl ConcSemispaceTLAB {
    #[cold]
    unsafe fn empty_buffer(&mut self, heap: &mut ConcSemispace) {
        let mut cursor = &mut self.buffer[0] as *mut *mut HeapObjectHeader;
        heap.write_barrier_lock.lock();
        while cursor < self.buffer_end {
            heap.write_barrier_buffer.push(cursor.read());
            cursor = cursor.add(1);
        }
        heap.write_barrier_lock.unlock();
        self.buffer_cursor = &mut self.buffer[0];
    }
}

impl ConcSemispace {
    #[inline]
    unsafe fn perform_write_barrier(
        &mut self,
        tlab: &mut ConcSemispaceTLAB,
        object: *mut HeapObjectHeader,
    ) {
        // incremental update barrier
        if !(*object).set_color(GC_BLACK, GC_GREY) {
            if tlab.buffer_cursor == tlab.buffer_end {
                tlab.empty_buffer(self);
            }

            tlab.buffer_cursor.write(object);
            tlab.buffer_cursor = tlab.buffer_cursor.add(1);
        }
    }
}

impl TLAB<ConcSemispace> for ConcSemispaceTLAB {
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

    fn refill(&mut self, mutator: &MutatorRef<ConcSemispace>, _size: usize) -> bool {
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
    fn create(heap: Arc<UnsafeCell<ConcSemispace>>) -> Self {
        let mut this = Self {
            heap,
            tlab_start: null_mut(),
            tlab_cursor: null_mut(),
            tlab_end: null_mut(),
            buffer: Box::new([null_mut(); TLAB_WB_BUFFER_SIZE]),
            buffer_cursor: null_mut(),
            buffer_end: null_mut(),
            is_conc_mark: false,
        };
        this.buffer_cursor = &mut this.buffer[0];
        this.buffer_end = &mut this.buffer[TLAB_WB_BUFFER_SIZE - 1];
        this
    }
}

impl GcBase for ConcSemispace {
    const SUPPORTS_TLAB: bool = true;
    type TLAB = ConcSemispaceTLAB;
    fn add_constraint<T: crate::gc_base::MarkingConstraint>(&mut self, _constraint: T) {
        todo!()
    }
    fn allocate_weak<T: Collectable + ?Sized>(
        &mut self,
        _mutator: &mut MutatorRef<Self>,
        _value: Gc<T>,
    ) -> crate::api::Weak<T> {
        todo!()
    }
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
            self.collect_alloc_failure(mutator, &mut [&mut value]);
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
        mutator: &mut MutatorRef<Self>,
        value: T,
    ) -> crate::api::Gc<T> {
        self.alloc_inline(mutator, value, AllocationSpace::Large)
    }
    fn collect(&mut self, _mutator: &mut MutatorRef<Self>, _keep: &mut [&mut dyn Trace]) {
        /*match SafepointScope::new(mutator.clone()) {
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
        }*/

        todo!()
    }
}
