use crate::api::*;

/// Shadow stack implementation. Internally this is singly-linked list of on stack rooted values.
pub struct ShadowStack {
    #[doc(hidden)]
    pub head: core::cell::Cell<*mut RawShadowStackEntry>,
}
impl ShadowStack {
    /// Create new shadow stack instance.
    pub fn new() -> Self {
        Self {
            head: core::cell::Cell::new(core::ptr::null_mut()),
        }
    }
    /// Walk all rooted values in this shadow stack.
    ///
    /// # Safety
    /// TODO: I don't really know if this method should be safe or unsafe.
    ///
    pub unsafe fn walk(&self, mut visitor: impl FnMut(&mut dyn Rootable)) {
        let mut head = *self.head.as_ptr();
        while !head.is_null() {
            let next = (*head).prev;
            visitor((*head).get_dyn());
            head = next;
        }
    }
}
/// Raw entry in GC shadow stack. Internal fields is not exposed in public API in any ways.
///
///
/// This type internally stores shadow stack pointeter,previous pointer from the list and vtable
/// that is used to construct `dyn` trait.
///
#[repr(C)]
pub struct RawShadowStackEntry {
    /// Shadowstack itself
    stack: *mut ShadowStack,
    /// Previous rooted entry
    prev: *mut RawShadowStackEntry,
    /// Pointer to vtable that is a `Trace` of rooted variable
    vtable: usize,
    /// Value is located right after vtable pointer, to access it we can construct trait object.
    data_start: [u8; 0],
}
/// Trait that should be implemented for all types that could be rooted.
/// In simple cases `impl<T: Traceable> Rootable for T {}` is enough.
pub trait Rootable: Trace {}
impl RawShadowStackEntry {
    /// Obtain mutable reference to rooted value.
    ///
    /// # Safety
    /// This method is `&self` but returns `&mut dyn` which is *very* unsafey. If moving GC uses shadow stack
    /// it should be ***very*** accurate when moving objects around.
    pub unsafe fn get_dyn(&self) -> &mut dyn Rootable {
        core::mem::transmute(crate::mopa::TraitObject {
            vtable: self.vtable as _,
            data: self.data_start.as_ptr() as *mut (),
        })
    }
}
/// Almost the same as raw entry of shadow stack except this one gives access to value.
/// This type is not exposed in public API and used only internally.
#[repr(C)]
pub struct ShadowStackInternal<'a, T: Rootable> {
    pub stack: &'a ShadowStack,
    pub prev: *mut RawShadowStackEntry,
    pub vtable: usize,
    pub value: T,
}
impl<'a, T: Rootable> ShadowStackInternal<'a, T> {
    #[doc(hidden)]
    /// Constructs internal shadow stack value. Must not be used outside of `$letroot!` macro.
    ///
    /// # Safety
    ///
    /// Very unsafe function and must not be used by users!
    #[inline]
    pub unsafe fn construct(
        stack: &'a ShadowStack,
        prev: *mut RawShadowStackEntry,
        vtable: usize,
        value: T,
    ) -> Self {
        Self {
            stack,
            prev,
            vtable,
            value,
        }
    }
}
impl<T: Rootable> Drop for ShadowStackInternal<'_, T> {
    /// Drop current shadow stack entry and update shadow stack state.
    #[inline(always)]
    fn drop(&mut self) {
        (*self.stack).head.set(self.prev);
    }
}
/// Rooted value on stack. This is non-copyable type that is used to hold GC thing on stack.
pub struct Rooted<'a, T: Rootable> {
    #[doc(hidden)]
    value: &'a mut T,
}
impl<'a, T: Rootable> Rooted<'a, T> {
    /// Create rooted value from pinned reference. Note that this function must be used only
    /// inside `$letroot` macro.
    ///
    /// # Safety
    ///
    ///  Very unsafe function and must not be used by users!
    pub unsafe fn construct(ptr: &'a mut T) -> Self {
        Self { value: ptr }
    }
}
impl<'a, T: Rootable> core::ops::Deref for Rooted<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.value
    }
}
impl<'a, T: Rootable> core::ops::DerefMut for Rooted<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value
    }
}

impl<T: Trace> Rootable for T {}
impl<T: Rootable> std::fmt::Pointer for Rooted<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:p}", self.value)
    }
}

/// Create rooted value and push it to provided shadowstack instance.
///
///
/// ***NOTE***: This macro does not heap allocate internally. It uses some unsafe tricks to
/// allocate value on stack and push stack reference to shadowstack. Returned rooted value internally
/// is `Pin<&mut T>`.
///
#[macro_export]
macro_rules! letroot {
    ($var_name: ident: $t: ty  = $stack: expr,$value: expr) => {
        let stack: &$crate::api::ShadowStack = &$stack;
        let value = $value;
        let mut $var_name = unsafe {
            $crate::api::ShadowStackInternal::<$t>::construct(
                stack,
                stack.head.get(),
                core::mem::transmute::<_, $crate::mopa::TraitObject>(
                    &value as &dyn $crate::api::Rootable,
                )
                .vtable as usize,
                value,
            )
        };

        stack
            .head
            .set(unsafe { core::mem::transmute(&mut $var_name) });
        #[allow(unused_mut)]
        let mut $var_name = unsafe { $crate::api::Rooted::construct(&mut $var_name.value) };
    };

    ($var_name : ident = $stack: expr,$value: expr) => {
        let stack: &$crate::shadow_stack::ShadowStack = &$stack;
        let value = $value;
        let mut $var_name = unsafe {
            $crate::shadow_stack::ShadowStackInternal::<_>::construct(
                stack,
                stack.head.get(),
                core::mem::transmute::<_, $crate::mopa::TraitObject>(
                    &value as &dyn $crate::shadow_stack::Rootable,
                )
                .vtable as usize,
                value,
            )
        };

        stack
            .head
            .set(unsafe { core::mem::transmute(&mut $var_name) });
        #[allow(unused_mut)]
        let mut $var_name =
            unsafe { $crate::shadow_stack::Rooted::construct(&mut $var_name.value) };
    };
}
