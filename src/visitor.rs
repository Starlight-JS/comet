use crate::{
    gc_info_table::GC_TABLE,
    gcref::{GcRef, UntypedGcRef},
    internal::trace_trait::{TraceDescriptor, TraceTrait},
};

pub trait VisitorTrait {
    fn visit(&mut self, this: *const u8, descriptor: TraceDescriptor) {
        let _ = this;
        let _ = descriptor;
    }

    fn visit_conservative(&mut self, from: *const *const u8, to: *const *const u8);
}

#[repr(C)]
pub struct Visitor {
    pub(crate) vis: *mut dyn VisitorTrait,
}

impl Visitor {
    /// Trace method for raw pointers. Prefer the versions for managed pointers.
    pub unsafe fn trace<T: TraceTrait>(&mut self, t: *const T) {
        if t.is_null() {
            return;
        }

        (*self.vis).visit(t.cast(), <T as TraceTrait>::get_trace_descriptor(t.cast()))
    }

    /// Trace method for inlined objects that are not allocated themselves but
    /// otherwise follow managed heap layout and have a trace() method.
    pub fn trace_ref<T: TraceTrait>(&mut self, object: &T) {
        <T as TraceTrait>::trace(object, self);
    }

    pub fn trace_gcref<T: TraceTrait>(&mut self, object: GcRef<T>) {
        unsafe {
            self.trace(object.downcast().get() as *mut T);
        }
    }

    pub fn trace_untyped<T: TraceTrait>(&mut self, object: UntypedGcRef) {
        unsafe {
            let header = &*object.header.as_ptr();
            let gc_info = GC_TABLE.get_gc_info(header.get_gc_info_index());
            (*self.vis).visit(
                object.get(),
                TraceDescriptor {
                    base_object_payload: object.get(),
                    callback: gc_info.trace,
                },
            )
        }
    }

    pub fn trace_conservatively(&mut self, from: *const u8, to: *const u8) {
        unsafe {
            (*self.vis).visit_conservative(from.cast(), to.cast());
        }
    }
}
