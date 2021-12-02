//! Immutable GCed utf-8 string
use std::mem::size_of;
use std::ops::Deref;
use std::{fmt, ops::Index};

use crate::{
    api::{Collectable, Finalize, Gc, Trace},
    base::GcBase,
};

#[repr(C)]
pub struct GcStr {
    length: usize,
    start: [u8; 0],
}

impl GcStr {
    pub fn new(heap: &mut crate::Heap, from: impl AsRef<str>) -> Gc<Self> {
        let string_ = from.as_ref().as_bytes();

        heap.allocate_and_init(
            Self {
                length: string_.len(),
                start: [0; 0],
            },
            |mut string| unsafe {
                std::ptr::copy_nonoverlapping(string_.as_ptr(), string.as_mut_ptr(), string.length);
            },
        )
    }
    pub fn from_utf8(
        heap: &mut crate::Heap,
        bytes: &[u8],
    ) -> Result<Gc<Self>, std::str::Utf8Error> {
        let string = std::str::from_utf8(bytes)?;
        Ok(Self::new(heap, string))
    }
    pub fn as_bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.as_ptr(), self.length) }
    }
    pub fn as_str(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(self.as_bytes()) }
    }
    pub fn len(&self) -> usize {
        self.length
    }

    pub fn as_ptr(&self) -> *const u8 {
        let start = &self.start as *const [u8; 0] as *const u8;
        start
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.as_ptr() as _
    }

    pub fn replace(
        &self,
        heap: &mut crate::Heap,
        from: impl AsRef<str>,
        to: impl AsRef<str>,
    ) -> Gc<GcStr> {
        let this = self.as_str();
        Self::new(heap, this.replace(from.as_ref(), to.as_ref()))
    }
}

impl fmt::Debug for GcStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl fmt::Display for GcStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Index<usize> for GcStr {
    type Output = u8;
    fn index(&self, index: usize) -> &Self::Output {
        &self.as_bytes()[index]
    }
}

impl AsRef<str> for GcStr {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for GcStr {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl Collectable for GcStr {
    fn allocation_size(&self) -> usize {
        size_of::<Self>() + self.length
    }
}

unsafe impl Finalize for GcStr {}
unsafe impl Trace for GcStr {}
