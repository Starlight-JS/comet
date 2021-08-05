pub type FinalizationCallback = extern "C" fn(*mut u8);

pub trait FinalizeTrait<T> {
    const NON_TRIVIAL_DTOR: bool = core::mem::needs_drop::<T>();
    const CALLBACK: Option<FinalizationCallback> = if Self::NON_TRIVIAL_DTOR {
        Some(Self::finalize)
    } else {
        None
    };

    extern "C" fn finalize(obj: *mut u8) {
        unsafe {
            core::ptr::drop_in_place(obj.cast::<T>());
        }
    }
}
