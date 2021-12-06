//! Relatively safe shadow-stack implementation.

use std::{marker::PhantomData, ptr::null_mut};

use crate::api::{vtable_of_trace, Trace};

pub struct LocalScope {
    pub(crate) next: *mut LocalScope,
    pub(crate) prev: *mut LocalScope,
    head: *mut RawLocal,
}
impl Drop for LocalScope {
    fn drop(&mut self) {
        unsafe {
            if !self.next.is_null() {
                (*self.next).prev = self.prev;
            }
        }
    }
}
#[repr(C, align(8))]
struct RawLocal {
    next: *mut RawLocal,
    prev: *mut RawLocal,
    vtable: usize,
    vptr: usize,
    value: [u8; 0],
}

#[repr(C, align(8))]
pub struct Local<'a, T: Trace> {
    next: *mut RawLocal,
    prev: *mut RawLocal,
    vtable: usize,
    vptr: usize,
    value: Option<T>,
    marker: PhantomData<&'a T>,
}

impl<'a, T: Trace> Drop for Local<'a, T> {
    fn drop(&mut self) {
        unsafe {
            if !self.next.is_null() {
                (*self.next).prev = self.prev;
            }
            if !self.prev.is_null() {
                (*self.prev).next = self.next;
            }
        }
    }
}

impl<'a, T: Trace> Local<'a, T> {
    pub unsafe fn construct(value: T) -> Self {
        Self {
            next: null_mut(),
            prev: null_mut(),
            vtable: vtable_of_trace::<T>(),
            vptr: 0,
            value: Some(value),
            marker: PhantomData,
        }
    }
    pub fn take(mut self) -> T {
        self.value.take().unwrap()
    }
}

impl LocalScope {
    pub unsafe fn add<T: Trace>(&mut self, local: &mut Local<'_, T>) {
        let raw = std::mem::transmute::<_, &mut RawLocal>(local);
        raw.next = self.head;
        self.head = raw as *mut _;
    }
}
