use crate::visitor::Visitor;

pub type TraceCallback = extern "C" fn(*mut Visitor, *const u8);
impl TraceTrait for u32 {}
impl TraceTrait for u16 {}
impl TraceTrait for u8 {}
pub trait TraceTrait: Sized {
    fn trace(&self, vis: &mut Visitor) {
        let _ = vis;
    }

    extern "C" fn trace_(vis: *mut Visitor, this: *const u8) {
        unsafe {
            (*this.cast::<Self>()).trace(&mut *vis);
        }
    }
    fn get_trace_descriptor(this: *const u8) -> TraceDescriptor {
        TraceDescriptor {
            base_object_payload: this,
            callback: Self::trace_,
        }
    }
}

pub struct TraceDescriptor {
    pub base_object_payload: *const u8,
    pub callback: TraceCallback,
}
