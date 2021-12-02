use im::Vector;

use crate::api::{Collectable, Gc, HandleMut, HeapObjectHeader, ShadowStack, Trace};

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

    fn shadow_stack<'a>(&self) -> &'a ShadowStack;

    fn allocate<T: Collectable + 'static>(&mut self, value: T) -> Gc<T>;
    fn allocate_and_init<T: Collectable + 'static + Unpin, F>(&mut self, value: T, init: F) -> Gc<T>
    where
        F: FnOnce(HandleMut<'_, T>),
    {
        let stack = self.shadow_stack();
        letroot!(value = stack, self.allocate(value));
        init(value.handle_mut());
        *value
    }

    fn allocate_safe<T: Collectable + 'static>(
        &mut self,
        value: T,
        refs: &mut [&mut dyn Trace],
    ) -> Gc<T>;
    fn try_allocate<T: Collectable + 'static>(&mut self, value: T) -> Result<Gc<T>, T>;

    /// Triggers garbage collection cycle. It is up to GC impl to decide whether to do full or minor cycle.
    fn collect(&mut self, refs: &mut [&mut dyn Trace]);

    fn minor_collection(&mut self, refs: &mut [&mut dyn Trace]) {
        self.collect(refs);
    }
    fn full_collection(&mut self, refs: &mut [&mut dyn Trace]) {
        self.collect(refs);
    }

    /// Registers object as finalizable. This function should be used when you want to execute finalizer
    /// even when `needs_drop::<T>()` returns false.
    fn register_finalizer<T: Collectable + ?Sized>(&mut self, object: Gc<T>);
}
