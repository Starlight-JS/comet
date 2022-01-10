use std::{
    char::decode_utf16,
    hash::Hash,
    mem::size_of,
    ops::{Deref, DerefMut, RangeBounds},
    str::Utf8Error,
};

use comet::letroot;

use super::vector::Vector;
use crate::{
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::GcBase,
    mutator::MutatorRef,
};
/// A possible error value when converting a `String` from a UTF-8 byte vector.
///
/// This type is the error type for the [`from_utf8`] method on [`String`]. It
/// is designed in such a way to carefully avoid reallocations: the
/// [`into_bytes`] method will give back the byte vector that was used in the
/// conversion attempt.
///
/// [`from_utf8`]: String::from_utf8
/// [`into_bytes`]: FromUtf8Error::into_bytes
///
/// The [`Utf8Error`] type provided by [`std::str`] represents an error that may
/// occur when converting a slice of [`u8`]s to a [`&str`]. In this sense, it's
/// an analogue to `FromUtf8Error`, and you can get one from a `FromUtf8Error`
/// through the [`utf8_error`] method.
///
/// [`Utf8Error`]: str::Utf8Error "std::str::Utf8Error"
/// [`std::str`]: core::str "std::str"
/// [`&str`]: prim@str "&str"
/// [`utf8_error`]: FromUtf8Error::utf8_error
///
/// # Examples
///
/// Basic usage:
///
/// ```
/// // some invalid bytes, in a vector
/// let bytes = vec![0, 159];
///
/// let value = String::from_utf8(bytes);
///
/// assert!(value.is_err());
/// assert_eq!(vec![0, 159], value.unwrap_err().into_bytes());
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct FromUtf8Error<H: GcBase> {
    bytes: Vector<u8, H>,
    error: Utf8Error,
}

#[derive(Debug)]
pub struct FromUtf16Error(());

/// GCed version of [alloc::string::String] It has all the same features as std String.
pub struct String<H: GcBase> {
    vec: Vector<u8, H>,
}

impl<H: GcBase> String<H> {
    /// Creates a new empty `String`.
    #[inline]
    pub fn new(mutator: &mut MutatorRef<H>) -> Self {
        Self {
            vec: Vector::new(mutator),
        }
    }
    /// Creates a new empty `String` with a particular capacity.
    ///
    /// `String`s have an internal buffer to hold their data. The capacity is
    /// the length of that buffer, and can be queried with the [`capacity`]
    /// method. This method creates an empty `String`, but one with an initial
    /// buffer that can hold `capacity` bytes. This is useful when you may be
    /// appending a bunch of data to the `String`, reducing the number of
    /// reallocations it needs to do.
    #[inline]
    pub fn with_capacity(mutator: &mut MutatorRef<H>, capacity: usize) -> Self {
        Self {
            vec: Vector::with_capacity(mutator, capacity),
        }
    }
    /// Converts a vector of bytes to a `String`.
    ///
    /// A string ([`String`]) is made of bytes ([`u8`]), and a vector of bytes
    /// ([`Vector<u8>`]) is made of bytes, so this function converts between the
    /// two. Not all byte slices are valid `String`s, however: `String`
    /// requires that it is valid UTF-8. `from_utf8()` checks to ensure that
    /// the bytes are valid UTF-8, and then does the conversion.
    ///
    /// If you are sure that the byte slice is valid UTF-8, and you don't want
    /// to incur the overhead of the validity check, there is an unsafe version
    /// of this function, [`from_utf8_unchecked`], which has the same behavior
    /// but skips the check.
    ///
    /// This method will take care to not copy the vector, for efficiency's
    /// sake.
    ///
    /// If you need a [`&str`] instead of a `String`, consider
    /// [`str::from_utf8`].
    ///
    /// The inverse of this method is [`into_bytes`].
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if the slice is not UTF-8 with a description as to why the
    /// provided bytes are not UTF-8. The vector you moved in is also included.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// // some bytes, in a vector
    /// let sparkle_heart = vec![240, 159, 146, 150];
    ///
    /// // We know these bytes are valid, so we'll use `unwrap()`.
    /// let sparkle_heart = String::from_utf8(sparkle_heart).unwrap();
    ///
    /// assert_eq!("💖", sparkle_heart);
    /// ```
    ///
    /// Incorrect bytes:
    ///
    /// ```
    /// // some invalid bytes, in a vector
    /// let sparkle_heart = vec![0, 159, 146, 150];
    ///
    /// assert!(String::from_utf8(sparkle_heart).is_err());
    /// ```
    ///
    /// See the docs for [`FromUtf8Error`] for more details on what you can do
    /// with this error.
    #[inline]
    pub fn from_utf8(vec: Vector<u8, H>) -> Result<Self, FromUtf8Error<H>> {
        match std::str::from_utf8(vec.as_slice()) {
            Ok(..) => Ok(String { vec }),
            Err(e) => Err(FromUtf8Error {
                bytes: vec,
                error: e,
            }),
        }
    }

    pub fn from_utf16(mutator: &mut MutatorRef<H>, v: &[u16]) -> Result<String<H>, FromUtf16Error> {
        let stack = mutator.shadow_stack();
        letroot!(ret = stack, Some(Self::with_capacity(mutator, v.len())));

        for c in decode_utf16(v.iter().copied()) {
            if let Ok(c) = c {
                ret.as_mut().unwrap().push(mutator, c);
            } else {
                return Err(FromUtf16Error(()));
            }
        }
        Ok(ret.take().unwrap())
    }

    #[inline]
    pub unsafe fn from_utf8_unchecked(bytes: Vector<u8, H>) -> Self {
        Self { vec: bytes }
    }

    #[inline]
    pub fn into_bytes(self) -> Vector<u8, H> {
        self.vec
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        self
    }

    #[inline]
    pub fn as_mut_str(&mut self) -> &mut str {
        self
    }

    #[inline]
    pub fn push_str(&mut self, mutator: &mut MutatorRef<H>, string: &str) {
        for byte in string.as_bytes() {
            self.vec.push(mutator, *byte);
        }
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.vec.capacity()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    #[inline]
    pub fn reserve(&mut self, mutator: &mut MutatorRef<H>, additional: usize) {
        self.vec.reserve(mutator, additional);
    }

    #[inline]
    pub fn push(&mut self, mutator: &mut MutatorRef<H>, ch: char) {
        match ch.len_utf8() {
            1 => self.vec.push(mutator, ch as u8),
            _ => {
                let mut dst = [0; 4];
                let utf8 = ch.encode_utf8(&mut dst).as_bytes();
                for x in utf8 {
                    self.vec.push(mutator, *x);
                }
            }
        }
    }
    #[inline]
    pub fn remove(&mut self, idx: usize) -> char {
        let ch = match self[idx..].chars().next() {
            Some(ch) => ch,
            None => panic!("cannot remove a char from the end of a string"),
        };

        let next = idx + ch.len_utf8();
        let len = self.len();
        unsafe {
            std::ptr::copy(
                self.vec.as_ptr().add(next),
                self.vec.as_mut_ptr().add(idx),
                len - next,
            );
            self.vec.set_len(len - (next - idx));
        }
        ch
    }
    #[inline]
    pub fn insert(&mut self, mutator: &mut MutatorRef<H>, idx: usize, ch: char) {
        assert!(self.is_char_boundary(idx));
        let mut bits = [0; 4];
        let bits = ch.encode_utf8(&mut bits).as_bytes();

        unsafe {
            self.insert_bytes(mutator, idx, bits);
        }
    }

    unsafe fn insert_bytes(&mut self, mutator: &mut MutatorRef<H>, idx: usize, bytes: &[u8]) {
        let len = self.len();
        let amt = bytes.len();
        self.vec.reserve(mutator, amt);

        std::ptr::copy(
            self.vec.as_ptr().add(idx),
            self.vec.as_mut_ptr().add(idx + amt),
            len - idx,
        );
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.vec.as_mut_ptr().add(idx), amt);
        self.vec.set_len(len + amt);
    }
    #[inline]
    pub fn insert_str(&mut self, mutator: &mut MutatorRef<H>, idx: usize, string: &str) {
        assert!(self.is_char_boundary(idx));

        unsafe {
            self.insert_bytes(mutator, idx, string.as_bytes());
        }
    }

    #[inline]
    pub unsafe fn as_mut_vec(&mut self) -> &mut Vector<u8, H> {
        &mut self.vec
    }

    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[inline]
    #[must_use = "use `.truncate()` if you don't need the other half"]
    pub fn split_off(&mut self, mutator: &mut MutatorRef<H>, at: usize) -> String<H> {
        assert!(self.is_char_boundary(at));
        let other = self.vec.split_off(mutator, at);
        unsafe { String::from_utf8_unchecked(other) }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.vec.clear()
    }
    pub fn replace_range<R>(&mut self, mutator: &mut MutatorRef<H>, range: R, replace_with: &str)
    where
        R: RangeBounds<usize>,
    {
        // Memory safety
        //
        // Replace_range does not have the memory safety issues of a vector Splice.
        // of the vector version. The data is just plain bytes.

        // WARNING: Inlining this variable would be unsound (#81138)
        let start = range.start_bound();
        match start {
            std::ops::Bound::Included(&n) => assert!(self.is_char_boundary(n)),
            std::ops::Bound::Excluded(&n) => assert!(self.is_char_boundary(n + 1)),
            std::ops::Bound::Unbounded => {}
        };
        // WARNING: Inlining this variable would be unsound (#81138)
        let end = range.end_bound();
        match end {
            std::ops::Bound::Included(&n) => assert!(self.is_char_boundary(n + 1)),
            std::ops::Bound::Excluded(&n) => assert!(self.is_char_boundary(n)),
            std::ops::Bound::Unbounded => {}
        };

        // Using `range` again would be unsound (#81138)
        // We assume the bounds reported by `range` remain the same, but
        // an adversarial implementation could change between calls
        unsafe { self.as_mut_vec() }.splice(mutator, (start, end), replace_with.bytes());
    }
}

unsafe impl<H: GcBase> Trace for String<H> {
    fn trace(&mut self, vis: &mut dyn crate::api::Visitor) {
        self.vec.trace(vis);
    }
}

unsafe impl<H: GcBase> Finalize for String<H> {}
impl<H: GcBase + 'static> Collectable for String<H> {}

impl<H: GcBase> Deref for String<H> {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        unsafe { std::str::from_utf8_unchecked(self.vec.as_slice()) }
    }
}

impl<H: GcBase> DerefMut for String<H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { std::str::from_utf8_unchecked_mut(self.vec.as_slice_mut()) }
    }
}

impl<H: GcBase> std::fmt::Debug for String<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}
impl<H: GcBase> std::fmt::Display for String<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, " {}", self.as_str())
    }
}

impl<H: GcBase> std::cmp::PartialEq for String<H> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str().eq(other.as_str())
    }
}
impl<H: GcBase> Eq for String<H> {}

impl<H: GcBase> Hash for String<H> {
    fn hash<HS: std::hash::Hasher>(&self, state: &mut HS) {
        self.as_str().hash(state);
    }
}

impl<H: GcBase> std::cmp::PartialOrd for String<H> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.as_str().partial_cmp(other.as_str())
    }
}

impl<H: GcBase> std::cmp::Ord for String<H> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

/// Garbage collected immuable string. It is just [str] that is allocated on GC heap.
#[repr(C)]
pub struct Str {
    length: usize,
    data_start: [u8; 0],
}

impl Str {
    pub fn new<H: GcBase>(mutator: &mut MutatorRef<H>, from: impl AsRef<str>) -> Gc<Self, H> {
        let src = from.as_ref();
        let mut this = mutator.allocate(
            Self {
                length: src.len(),
                data_start: [],
            },
            crate::gc_base::AllocationSpace::New,
        );
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr(), this.data_start.as_mut_ptr(), src.len());
        }
        this
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn as_str(&self) -> &str {
        self
    }

    pub fn as_mut_str(&mut self) -> &mut str {
        self
    }
}

impl Deref for Str {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                self.data_start.as_ptr(),
                self.len(),
            ))
        }
    }
}

impl DerefMut for Str {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            std::str::from_utf8_unchecked_mut(std::slice::from_raw_parts_mut(
                self.data_start.as_mut_ptr(),
                self.len(),
            ))
        }
    }
}
unsafe impl Trace for Str {}
unsafe impl Finalize for Str {}
impl Collectable for Str {
    fn allocation_size(&self) -> usize {
        size_of::<Self>() + self.length
    }
}

impl Hash for Str {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl Eq for Str {}

impl PartialEq for Str {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl std::fmt::Display for Str {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::fmt::Debug for Str {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
