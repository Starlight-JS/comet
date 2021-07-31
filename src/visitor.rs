use crate::internal::trace_trait::{TraceDescriptor, TraceTrait};

pub trait VisitorTrait {
    fn visit(&mut self, this: *const u8, descriptor: TraceDescriptor) {
        let _ = this;
        let _ = descriptor;
    }

    fn visit_weak(&mut self, this: *const u8, descriptor: TraceDescriptor) {}
}

#[repr(C)]
pub struct Visitor {
    vis: *mut dyn VisitorTrait,
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
}
