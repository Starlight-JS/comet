//! Immutable GCed utf-8 string
use std::marker::PhantomData;
use std::mem::size_of;
use std::ops::Deref;
use std::{fmt, ops::Index};

use crate::{
    api::{Collectable, Finalize, Gc, Trace},
    base::GcBase,
};

#[repr(C)]
pub struct GcStr<H: GcBase = crate::Heap> {
    length: usize,
    marker: PhantomData<H>,
    start: [u8; 0],
}

impl<H: GcBase + 'static> GcStr<H> {
    pub fn new(heap: &mut H, from: impl AsRef<str>) -> Gc<Self>
    where
        H: Unpin,
    {
        let string_ = from.as_ref().as_bytes();

        heap.allocate_and_init(
            Self {
                marker: PhantomData,
                length: string_.len(),
                start: [0; 0],
            },
            |string| unsafe {
                std::ptr::copy_nonoverlapping(string_.as_ptr(), string.as_mut_ptr(), string.length);
            },
        )
    }
    pub fn from_utf8(heap: &mut H, bytes: &[u8]) -> Result<Gc<Self>, std::str::Utf8Error>
    where
        H: Unpin,
    {
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

    pub fn replace(&self, heap: &mut H, from: impl AsRef<str>, to: impl AsRef<str>) -> Gc<Self>
    where
        H: Unpin,
    {
        let this = self.as_str();
        Self::new(heap, this.replace(from.as_ref(), to.as_ref()))
    }
}

impl<H: GcBase + 'static> fmt::Debug for GcStr<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl<H: GcBase + 'static> fmt::Display for GcStr<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl<H: GcBase + 'static> Index<usize> for GcStr<H> {
    type Output = u8;
    fn index(&self, index: usize) -> &Self::Output {
        &self.as_bytes()[index]
    }
}

impl<H: GcBase + 'static> AsRef<str> for GcStr<H> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<H: GcBase + 'static> Deref for GcStr<H> {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl<H: GcBase + 'static> Collectable for GcStr<H> {
    fn allocation_size(&self) -> usize {
        size_of::<Self>() + self.length
    }
}

unsafe impl<H: GcBase + 'static> Finalize for GcStr<H> {}
unsafe impl<H: GcBase + 'static> Trace for GcStr<H> {}
