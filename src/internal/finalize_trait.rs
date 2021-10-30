pub type FinalizationCallback = extern "C" fn(*mut u8);

/// The FinalizeTrait specifies how to finalize objects.
pub trait FinalizeTrait<T> {
    /// If true finalizer is executed at the end of GC cycle for this type. 
    /// In most cases compiler should be smart enough to determine if `NON_TRIVIAL_DTOR` is true
    /// but in some rare unsafe cases you might set it to `false` by yourself.
    const NON_TRIVIAL_DTOR: bool = std::mem::needs_drop::<T>();
    /// Finalization callback executed by GC at the end of GC cycle. It defaults to [FinalizeTrait::finalize]. 
    const CALLBACK: Option<FinalizationCallback> = if Self::NON_TRIVIAL_DTOR {
        Some(Self::finalize)
    } else {
        None
    };
    /// Callback that is executed at the end of GC cycle. It invokes `T::drop` by default.
    extern "C" fn finalize(obj: *mut u8) {
        unsafe {
            core::ptr::drop_in_place(obj.cast::<T>());
        }
    }
}
