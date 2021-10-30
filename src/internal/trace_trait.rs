use crate::visitor::Visitor;

pub type TraceCallback = extern "C" fn(*mut Visitor, *const u8);

/// Trait specifying how the garbage collector processes an object of type `T`.
pub trait TraceTrait: Sized {
    /// Function invoking the tracing for an object of type `T`.
    /// - `visitor`: The visitor to dispatch to.
    fn trace(&self, vis: &mut Visitor) {
        let _ = vis;
    }
    /// `trace_` method is used for C FFI safety. 
    extern "C" fn trace_(vis: *mut Visitor, this: *const u8) {
        unsafe {
            (*this.cast::<Self>()).trace(&mut *vis);
        }
    }
    /// Returns trace descriptor for type that implements this trait. Not recommended to override it. 
    fn get_trace_descriptor(this: *const u8) -> TraceDescriptor {
        TraceDescriptor {
            base_object_payload: this,
            callback: Self::trace_,
        }
    }
}

/// Describe how to trace an object.
pub struct TraceDescriptor {
    pub base_object_payload: *const u8,
    pub callback: TraceCallback,
}
