use std::{
    any::TypeId,
    cell::UnsafeCell,
    marker::PhantomData,
    ptr::{null_mut, NonNull},
    sync::{atomic::AtomicUsize, Arc},
};

use crate::{
    api::{Collectable, Gc, HeapObjectHeader, Trace, Visitor, Weak},
    mutator::{Mutator, MutatorRef},
    rosalloc_space::RosAllocSpace,
    safepoint::GlobalSafepoint,
};

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum AllocationSpace {
    New,
    Old,
    Large,
}

pub struct NoHelp;

/// Base trait for all GCs.
pub trait GcBase: Sized + 'static {
    /// Default large object size. If allocation request exceeds this constant [GcBase::allocate_large] is invoked.
    const LARGE_ALLOCATION_SIZE: usize = 16 * 1024;
    /// Returns `true` if GC supports thread local allocation.
    const SUPPORTS_TLAB: bool = false;
    type ReadBarrier: ReadBarrier<Self>;
    type TLAB: TLAB<Self>;
    type TRAIT: Collectable + ?Sized = dyn Collectable;
    type InlineAllocationHelpers = NoHelp;

    fn inline_allocation_helpers(&self) -> Self::InlineAllocationHelpers;
    fn add_constraint<T: MarkingConstraint + 'static>(&mut self, constraint: T);
    fn inspect(&self, f: impl FnMut(Gc<dyn Collectable, Self>) -> bool) -> bool {
        let _ = f;
        false
    }
    /// Allocates `size` bytes on heap and creates object header with `type_id` and `vtable`. This function can be used to allocate dyn sized arrays or strings.
    fn allocate_raw(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        size: usize,
        type_id: TypeId,
        vtable: usize,
    ) -> *mut HeapObjectHeader {
        let _ = mutator;
        let _ = size;
        let _ = type_id;
        let _ = vtable;
        todo!()
    }
    /// Allocates weak reference on GC heap
    fn allocate_weak<T: Collectable + ?Sized>(
        &mut self,
        _mutator: &mut MutatorRef<Self>,
        _value: Gc<T, Self>,
    ) -> Weak<T, Self> {
        panic!(
            "Weak references are not supported by `{}`",
            std::any::type_name::<Self>()
        );
    }
    fn get_rosalloc_space(&self) -> *mut RosAllocSpace {
        null_mut()
    }
    /// Collect memory on allocation failure
    fn collect_alloc_failure(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        keep: &mut [&mut dyn Trace],
    ) {
        self.collect(mutator, keep);
    }
    /// Attach mutator to GC heap
    fn attach_current_thread(&mut self, mutator: *mut Mutator<Self>);
    /// Detach mutator from GC heap
    fn detach_current_thread(&mut self, mutator: *mut Mutator<Self>);

    /// Get safepoint reference
    fn safepoint(&self) -> &GlobalSafepoint;

    /// Acquire global heap lock
    fn global_lock(&self);
    /// Release global heap lock
    fn global_unlock(&self);
    /// Get mutators list.
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
        space: AllocationSpace,
    ) -> Gc<T, Self>;

    /// Post allocation operation e.g set mark in bitmap that this object was allocated.
    ///
    /// Restrictions for this function:
    /// - Must not acquire any mutex locks when `needs_drop::<T>()` returns false
    /// - Must not do CPU heavy operations
    /// - Must put `value` to finalizer list if `needs_drop::<T>()` returns true
    #[inline(always)]
    fn post_alloc<T: Collectable + Sized + 'static>(&mut self, value: Gc<T, Self>) {
        let _ = value;
    }
    /// Allocates large object in GC heap.
    fn allocate_large<T: Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        value: T,
    ) -> Gc<T, Self>;

    /// Perform minor GC cycle by stopping all threads and collecting unused memory.
    fn minor_collection(&mut self, mutator: &mut MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        self.full_collection(mutator, keep);
    }
    /// Perform full GC cycle by stopping all threads and collecting unused memory.
    fn full_collection(&mut self, mutator: &mut MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        self.collect(mutator, keep);
    }
    /// Perform garbage collection cycle by stopping all threads and collecting unused memory.
    fn collect(&mut self, mutator: &mut MutatorRef<Self>, keep: &mut [&mut dyn Trace]);

    /// Write barrier implementation. No-op by default.
    fn write_barrier(&mut self, mutator: &mut MutatorRef<Self>, object: Gc<dyn Collectable, Self>) {
        let _ = object;
        let _ = mutator;
    }
    /// Initialize TLAB
    fn init_tlab(&mut self, tlab: &mut Self::TLAB) {
        let _ = tlab;
    }
}

/// Thread local allocation buffer. Instances of TLAB usually store write barrier buffers and thread local allocators.
pub trait TLAB<H: GcBase<TLAB = Self>> {
    /// Can we allocate `size` bytes in thread local buffer?
    fn can_thread_local_allocate(&self, size: usize) -> bool;
    /// Allocate `value` in TLB or if there is no enough memory return `Err(value)`.
    fn allocate<T: Collectable + 'static>(&mut self, value: T) -> Result<Gc<T, H>, T>;
    /// Refill TLAB with new TLB. Returns `false` on failure.
    fn refill(&mut self, mutator: &MutatorRef<H>, alloc_size: usize) -> bool;
    /// Reset TLAB
    fn reset(&mut self);
    /// Create new TLAB instance.
    fn create(heap: Arc<UnsafeCell<H>>) -> Self;
}

/// Fill region from `start` to `end` with "free" object. If object is too large it is filled as [Array<i32>](crate::alloc::array::Array).
/// This code is useful when you want to iterate memory region for live objects without using bitmaps or other ways
/// of keeping information about live objects.
///
pub unsafe fn fill_region(start: *mut u8, end: *mut u8) {
    /*if start == end {
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
    }*/
    let _ = start;
    let _ = end;
    todo!()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkingConstraintRuns {
    AfterMark,
    BeforeMark,
}

/// Marking constraint that is to be executed at GC cycle.
///
/// # Safety
/// This trait is unsafe because in marking constraint some things are impossible to prove for safety:
/// - Panic should not happen in constraint. In case you really screwed up use [abort](std::process::abort).
/// - Calling into GC methods like allocation or triggering GC cycles
pub unsafe trait MarkingConstraint {
    /// Returns when marking constraint should run: after mark or before marking cycle.
    fn runs_at(&self) -> MarkingConstraintRuns;
    fn name(&self) -> &str;
    /// Returns `true` if this constraint is over and we do not want to execute it furthermore. Note
    /// that if this returns `true` we remove that constraint from constraint list.
    fn is_over(&self) -> bool;
    /// Executes this constraint.
    fn run(&mut self, visitor: &mut dyn Visitor);
}

pub trait ReadBarrier<H: GcBase>: Sized + 'static {
    fn read_barrier<T: Collectable + ?Sized>(x: Gc<T, H>) -> Gc<T, H> {
        x
    }
}

pub struct NoReadBarrier;

impl<H: GcBase> ReadBarrier<H> for NoReadBarrier {
    #[inline(always)]
    fn read_barrier<T: Collectable + ?Sized>(x: Gc<T, H>) -> Gc<T, H> {
        x
    }
}

/// Brooks pointer read barrier. In this read barrier we store forwarding pointer just behind
/// object header and this forwarding pointer points to the same location or to new location, it
/// is the cheapest read barrier out there.
pub struct BrooksPointer;

impl<H: GcBase> ReadBarrier<H> for BrooksPointer {
    #[inline(always)]
    fn read_barrier<T: Collectable + ?Sized>(x: Gc<T, H>) -> Gc<T, H> {
        unsafe {
            let base = x.base.as_ptr().cast::<AtomicUsize>().sub(1);
            Gc {
                base: NonNull::new_unchecked((*base).load(atomic::Ordering::Acquire) as _),
                marker: PhantomData,
            }
        }
    }
}
