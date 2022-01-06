use std::{
    char::decode_utf16,
    ops::{Deref, DerefMut},
    str::Utf8Error,
};

use super::vector::Vector;
use crate::{
    api::{Collectable, Finalize, Trace},
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
