use std::mem::MaybeUninit;

use im::Vector;

use crate::api::{Collectable, Gc, HeapObjectHeader, ShadowStack, Trace, Visitor};

/// A base trait for all garbage collector implementations.
pub trait GcBase {
    const MOVING_GC: bool = false;
    const NEEDS_WRITE_BARRIER: bool = false;
    const CAN_USUALLY_PIN_OBJECTS: bool = false;
    const OBJECT_MINIMAL_SIZE: usize = 0;
    const MAX_ALLOCATION_SIZE: usize = u32::MAX as _;
    fn finalize_handlers(&self) -> &Vector<*mut HeapObjectHeader>;
    fn finalize_handlers_mut(&mut self) -> &mut Vector<*mut HeapObjectHeader>;
    fn set_finalize_lock(&mut self, x: bool);
    fn finalize_lock(&self) -> bool;

    fn execute_finalizers(&mut self) {
        if self.finalize_lock() {
            return;
        }

        self.set_finalize_lock(true);
        // Ideally finalizer should not panic but just in case it panics we catch unwind.
        let _result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for item in self.finalize_handlers() {
                unsafe {
                    let object = (**item).get_dyn();
                    // Invoke object finalizer
                    object.finalize();
                }
            }
        }));
        self.set_finalize_lock(false);
    }

    /// Returns shadow stack reference that is valid only for scope lifetime
    fn shadow_stack<'a>(&self) -> &'a ShadowStack;

    /// Allocates `value` on GC heap.
    ///
    ///
    /// **NOTE**: Might trigger GC cycle if there is no enough memory or certain threshold is reached (depends on GC impl)
    fn allocate<T: Collectable + 'static>(&mut self, value: T) -> Gc<T>;
    fn allocate_and_init<T: Collectable + 'static + Unpin, F>(&mut self, value: T, init: F) -> Gc<T>
    where
        F: FnOnce(&'_ mut Gc<T>),
    {
        let stack = self.shadow_stack();
        letroot!(value = stack, self.allocate(value));
        init(&mut value);
        *value
    }
    /// Same as [GcBase::allocate] but also allows to pass some references as protected in case GC cycle happens.
    fn allocate_safe<T: Collectable + 'static>(
        &mut self,
        value: T,
        refs: &mut [&mut dyn Trace],
    ) -> Gc<T>;
    /// Tries to allocate `value` on GC heap, if there is no enough memory `Err(value)` is returned.
    fn try_allocate<T: Collectable + 'static>(&mut self, value: T) -> Result<Gc<T>, T>;
    /// Allocate raw memory for `T`. User is responsible for initializing it.
    fn allocate_raw<T: Collectable>(&mut self, size: usize) -> Option<Gc<MaybeUninit<T>>>;
    /// Triggers garbage collection cycle. It is up to GC impl to decide whether to do full or minor cycle.
    fn collect(&mut self, refs: &mut [&mut dyn Trace]);

    /// Minor collection cycle. By default invokes [GcBase::collect].
    fn minor_collection(&mut self, refs: &mut [&mut dyn Trace]) {
        self.collect(refs);
    }
    /// Full collection cycle. By default invokes [GcBase::collect].

    fn full_collection(&mut self, refs: &mut [&mut dyn Trace]) {
        self.collect(refs);
    }

    /// Registers object as finalizable. This function should be used when you want to execute finalizer
    /// even when `needs_drop::<T>()` returns false.
    fn register_finalizer<T: Collectable + ?Sized>(&mut self, object: Gc<T>);
    /// Write barrier implementation. By default it is no-op.
    #[inline(always)]
    fn write_barrier<T: Collectable + ?Sized>(&mut self, object: Gc<T>) {
        let _ = object;
    }

    /// Register task to run just before marking. Returns `usize` that can be used later to remove this task.    
    fn add_marking_task(&mut self, task: Box<dyn MarkingTask>) -> usize;
    //  fn add_local_scope(&mut self, scope: &mut LocalScope);
}

unsafe impl<T: Trace> Trace for [T] {
    fn trace(&mut self, _vis: &mut dyn crate::api::Visitor) {
        for x in self.iter_mut() {
            x.trace(_vis);
        }
    }
}

pub trait MarkingTask {
    fn run(&mut self, vis: &mut dyn Visitor);
}
