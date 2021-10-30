use crate::{
    gc_info_table::GC_TABLE,
    gcref::{GcRef, UntypedGcRef},
    heap::Heap,
    internal::trace_trait::{TraceDescriptor, TraceTrait},
};

pub trait VisitorTrait {
    /// Visits object with provided [TraceDescriptor]. 
    fn visit(&mut self, this: *const u8, descriptor: TraceDescriptor) {
        let _ = this;
        let _ = descriptor;
    }
    /// Visits objects in `from` to `to` range conservatively. This function will read gc info index from
    /// each object that is found in memory range and obtain TraceDescriptor from that.
    fn visit_conservative(&mut self, from: *const *const u8, to: *const *const u8);
    
    fn heap(&self) -> *mut Heap;
}

#[repr(C)]
pub struct Visitor {
    pub(crate) vis: *mut dyn VisitorTrait,
}

impl Visitor {
    pub(crate) fn heap(&self) -> *mut Heap {
        unsafe { (*self.vis).heap() }
    }
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
    /// Traces typed gc reference. 
    pub fn trace_gcref<T: TraceTrait>(&mut self, object: GcRef<T>) {
        unsafe {
            self.trace(object.downcast().get() as *mut T);
        }
    }
    /// Traces untyped gc reference.
    pub fn trace_untyped(&mut self, object: UntypedGcRef) {
        unsafe {
            let header = &*object.header.as_ptr();
            let gc_info = GC_TABLE.get_gc_info(header.get_gc_info_index());
            (*self.vis).visit(
                header.payload(),
                TraceDescriptor {
                    base_object_payload: header.payload(),
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
