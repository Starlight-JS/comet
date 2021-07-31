pub type FInalizationCallback = extern "C" fn(*mut u8);

pub trait FinalizeTrait<T> {
    const NON_TRIVIAL_DTOR: bool = core::mem::needs_drop::<T>();
    const CALLBACK: Option<FInalizationCallback> = if Self::NON_TRIVIAL_DTOR {
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

impl FinalizeTrait<u16> for u16 {}
impl FinalizeTrait<u32> for u32 {}
impl FinalizeTrait<u8> for u8 {}
