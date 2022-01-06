use std::{
    hash::Hash,
    hint::unreachable_unchecked,
    marker::PhantomData,
    mem::{size_of, MaybeUninit},
    ops::{Deref, DerefMut},
    ptr::{null_mut, NonNull},
    sync::atomic::AtomicU16,
};

use crate::{
    gc_base::{GcBase, ReadBarrier},
    large_space::PreciseAllocation,
    mutator::MutatorRef,
    small_type_id,
    utils::*,
};
use atomic::Ordering;
use mopa::mopafy;

/// Indicates that a type can be traced by a garbage collector.
///
/// This doesn't necessarily mean that the type is safe to allocate in a garbage collector ([Collectable]).
///
/// ## Safety
/// See the documentation of the `trace` method for more info.
/// Essentially, this object must faithfully trace anything that
/// could contain garbage collected pointers or other `Trace` items.
pub unsafe trait Trace {
    /// Trace each field in this type.
    ///
    /// Structures should trace each of their fields,
    /// and collections should trace each of their elements.
    ///
    /// ### Safety
    /// Some types (like `Gc`) need special actions taken when they're traced,
    /// but those are somewhat rare and are usually already provided by the garbage collector.
    ///
    /// Behavior is restricted during tracing:
    /// ## Permitted Behavior
    /// - Reading your own memory (includes iteration)
    ///   - Interior mutation is undefined behavior, even if you use `RefCell`
    /// - Calling `Visitor::mark_object`
    ///   
    /// - Panicking on unrecoverable errors
    ///   - This should be reserved for cases where you are seriously screwed up,
    ///       and can't fulfill your contract to trace your interior properly.
    ///     - One example is `Gc<T>` which panics if the garbage collectors are mismatched
    ///   - Garbage collectors may chose to [abort](std::process::abort) if they encounter a panic,
    ///     so you should avoid doing it if possible.
    /// ## Never Permitted Behavior
    /// - Forgetting a element of a collection, or field of a structure
    ///   - If you forget an element undefined behavior will result
    ///   - This is why you should always prefer automatically derived implementations where possible.
    ///     - With an automatically derived implementation you will never miss a field
    /// - It is undefined behavior to mutate any of your own data.
    ///   - The mutable `&mut self` is just so copying collectors can relocate GC pointers
    /// - Calling other operations on the garbage collector (including allocations)
    fn trace(&mut self, _vis: &mut dyn Visitor) {}
}
/// Indicates type that can be allocated on garbage collector heap.
pub trait Collectable: Trace + Finalize + mopa::Any {
    /// Function to compute value size on allocation. If type is dyn sized (i.e array or string) this function must be overloaded
    /// to calculate allocation size properly. It is invoked exactly once at allocation time.
    #[inline(always)]
    fn allocation_size(&self) -> usize {
        std::mem::size_of_val(self)
    }
}

mopafy!(Collectable);

pub unsafe trait Finalize {
    unsafe fn finalize(&mut self) {
        std::ptr::drop_in_place(self)
    }
}

pub const GC_WHITE: u8 = 0;
pub const GC_BLACK: u8 = 1;
pub const GC_GREY: u8 = 2;

/// HeapObjectHeader contains meta data per object and is prepended to each
/// object.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HeapObjectHeader {
    /// First 64 bit word of header. It stores object vtable that contains [Trace::trace] and [Finalize::finalize] methods.
    pub value: u64,
    /// Metadata stored there depends strictly on GC type
    pub padding: u16,
    /// Metadata stored there depends strictly on GC type
    pub padding2: u16,
    /// TypeId of allocated type that is again hashed by 32 bit hasher so object header is 2 words on 64 bit platforms.
    pub type_id: u32,
}

/// Minimal allocation size in GC heap.
pub const MIN_ALLOCATION: usize = 8;

impl HeapObjectHeader {
    #[inline]
    pub fn set_free(&mut self) {
        self.type_id = 0;
    }
    #[inline]
    pub fn is_free(&self) -> bool {
        self.type_id == 0
    }
    #[inline(always)]
    pub fn get_dyn(&mut self) -> &mut dyn Collectable {
        unsafe {
            std::mem::transmute::<_, _>(mopa::TraitObject {
                data: self.data() as *mut (),
                vtable: self.vtable() as _,
            })
        }
    }
    #[inline(always)]
    pub fn set_forwarded(&mut self, fwdptr: usize) {
        self.value = VTableBitField::encode(fwdptr as _);
        self.padding = ForwardedBit::encode(1) as _;
    }
    #[inline(always)]
    pub fn is_forwarded(&self) -> bool {
        ForwardedBit::decode(self.padding as _) != 0
    }

    #[inline(always)]
    pub fn size(&self) -> usize {
        SizeBitField::decode(self.padding2 as _) as usize * MIN_ALLOCATION
    }
    #[inline(always)]
    pub fn is_precise(&self) -> bool {
        SizeBitField::decode(self.padding2 as _) == 0
    }
    #[inline(always)]
    pub fn set_size(&mut self, size: usize) {
        //assert!(size != 0);
        self.padding2 =
            SizeBitField::update(self.padding2 as _, size as u64 / MIN_ALLOCATION as u64) as _;
    }
    #[inline(always)]
    pub fn set_large(&mut self) {
        self.padding2 = SizeBitField::update(self.padding2 as _, 0) as _;
    }
    #[inline(always)]
    pub fn vtable(&self) -> usize {
        VTableBitField::decode(self.value) as _
    }
    #[inline(always)]
    pub fn set_vtable(&mut self, vtable: usize) {
        self.value = VTableBitField::encode(vtable as _);
    }
    #[inline(always)]
    pub fn is_allocated(&self) -> bool {
        self.vtable() != 0
    }
    #[inline(always)]
    pub fn data(&self) -> *const u8 {
        ((self as *const Self as usize) + size_of::<Self>()) as *const u8
    }
    #[inline(always)]
    pub fn marked_bit(&self) -> bool {
        MarkBit::decode(self.padding as _) != 0
    }
    #[inline(always)]
    pub fn unmark(&mut self) {
        self.padding = MarkBit::update(self.padding as _, 0) as _;
    }
    #[inline(always)]
    pub fn set_marked_bit(&mut self) {
        self.padding = MarkBit::update(self.padding as _, 1) as _;
    }
    #[inline(always)]
    pub fn type_id(&self) -> u32 {
        self.type_id
    }

    #[inline]
    pub fn set_color(&self, current: u8, new: u8) -> bool {
        unsafe {
            let atomic = &*(&self.padding as *const u16 as *const AtomicU16);
            let word = atomic.load(atomic::Ordering::Relaxed);
            match atomic.compare_exchange_weak(
                ColourBit::update(word as _, current as _) as _,
                ColourBit::update(word as _, new as _) as _,
                atomic::Ordering::AcqRel,
                atomic::Ordering::Relaxed,
            ) {
                Ok(_) => false,
                Err(_) => true,
            }
        }
    }
    #[inline]
    pub fn force_set_color(&mut self, color: u8) {
        self.padding = ColourBit::update(self.padding as _, color as _) as _;
    }
    #[inline]
    pub fn get_color(&self) -> u8 {
        unsafe {
            let atomic = &*(&self.padding as *const u16 as *const AtomicU16);
            ColourBit::decode(atomic.load(Ordering::Relaxed) as _) as _
        }
    }
    #[inline(always)]
    pub fn parent_known_bit(&self) -> bool {
        ParentKnown::decode(self.padding as _) != 0
    }

    #[inline(always)]
    pub fn set_parent_known_bit(&mut self, bit: bool) {
        self.padding = ParentKnown::update(self.padding as _, bit as u64) as _;
    }

    #[inline(always)]
    pub fn pinned_bit(&self) -> bool {
        Pinned::decode(self.padding as _) != 0
    }

    #[inline(always)]
    pub fn set_pinned_bit(&mut self, bit: bool) {
        self.padding = Pinned::update(self.padding as _, bit as _) as _;
    }
}

unsafe impl<T: Collectable + ?Sized, H: GcBase> Finalize for Gc<T, H> {}
impl<T: Collectable + ?Sized, H: GcBase> Collectable for Gc<T, H> {}
pub(crate) fn vtable_of<T: Collectable>() -> usize {
    let x = null_mut::<T>();
    unsafe { std::mem::transmute::<_, mopa::TraitObject>(x as *mut dyn Collectable).vtable as _ }
}

#[allow(dead_code)]
pub(crate) fn vtable_of_trace<T: Trace>() -> usize {
    let x = null_mut::<T>();
    unsafe { std::mem::transmute::<_, mopa::TraitObject>(x as *mut dyn Trace).vtable as _ }
}
/// A garbage collected pointer to a value.
///
/// This is the equivalent of a garbage collected smart-pointer.
///
///
/// The smart pointer is simply a guarantee to the garbage collector
/// that this points to a garbage collected object with the correct header,
/// and not some arbitrary bits that you've decided to heap allocate.
///
pub struct Gc<T: Collectable + ?Sized, H: GcBase> {
    pub(crate) base: NonNull<HeapObjectHeader>,
    pub(crate) marker: PhantomData<(Box<T>, H)>,
}
impl<T: Collectable + Sized, H: GcBase> Gc<MaybeUninit<T>, H> {
    pub unsafe fn assume_init(self) -> Gc<T, H> {
        Gc {
            base: self.base,
            marker: Default::default(),
        }
    }
}
impl<T: Collectable + ?Sized, H: GcBase> Gc<T, H> {
    /// Coerce this GC pointer to dyn Collectable.
    #[inline]
    pub fn to_dyn(self) -> Gc<dyn Collectable, H> {
        Gc {
            base: self.base,
            marker: PhantomData,
        }
    }
    /// Check if this GC pointer is of type `U`
    #[inline(always)]
    pub fn is<U: Collectable>(&self) -> bool {
        unsafe { (*self.base.as_ptr()).type_id == small_type_id::<U>() }
    }
    /// Get type vtable
    #[inline]
    pub fn vtable(&self) -> usize {
        unsafe { (*self.base.as_ptr()).vtable() }
    }
    /// Try to downcast this reference to `U`
    #[inline]
    pub fn downcast<U: Collectable>(&self) -> Option<Gc<U, H>> {
        if self.is::<U>() {
            Some(Gc {
                base: self.base,
                marker: PhantomData,
            })
        } else {
            None
        }
    }

    /// Unchecked downcast
    ///
    /// # Safety
    /// Unsafe to call because does not check for type
    #[inline]
    pub unsafe fn downcast_unchecked<U: Collectable>(&self) -> Gc<U, H> {
        self.downcast().unwrap_or_else(|| unreachable_unchecked())
    }
    /// Returns number of bytes that this GC pointer uses on the heap.
    pub fn allocation_size(&self) -> usize {
        unsafe {
            let base = &*self.base.as_ptr();
            if base.is_precise() {
                (*PreciseAllocation::from_cell(self.base.as_ptr() as *mut _)).cell_size()
            } else {
                base.size()
            }
        }
    }
}

impl<T: Collectable + ?Sized, H: GcBase> Clone for Gc<T, H> {
    fn clone(&self) -> Self {
        H::ReadBarrier::read_barrier(*self)
    }
}
impl<T: Collectable + ?Sized, H: GcBase> Copy for Gc<T, H> {}

macro_rules! impl_prim {
    ($($t: ty)*) => {
        $(
            unsafe impl Trace for $t {}
            unsafe impl Finalize for $t {}
            impl Collectable for $t {}
        )*
    };
}

impl_prim!(
    u8 u16 u32 u64 u128
    i8 i16 i32 i64 i128
    f32 f64
    bool
    std::fs::File String

);

unsafe impl Trace for () {}
unsafe impl Finalize for () {}
impl Collectable for () {}
unsafe impl<T: Collectable + ?Sized, H: GcBase> Trace for Gc<T, H> {
    fn trace(&mut self, vis: &mut dyn Visitor) {
        vis.mark_object(&mut self.base);
    }
}

pub trait Visitor {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>);
    /// Callback to invoke when marking weak references. In most GC impls it is enough to simply invoke `mark_object`. But in some cases (e.g concurrent collector)
    /// it might have special cases where that is not enough.
    fn mark_weak(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        self.mark_object(root);
    }
}

impl<T: Collectable + ?Sized, H: GcBase> std::fmt::Pointer for Gc<T, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:p}", self.base)
    }
}
unsafe impl Trace for &mut [&mut dyn Trace] {
    fn trace(&mut self, _vis: &mut dyn Visitor) {
        for x in self.iter_mut() {
            x.trace(_vis);
        }
    }
}

unsafe impl<T: Trace> Trace for Option<T> {
    fn trace(&mut self, _vis: &mut dyn Visitor) {
        match self {
            Some(val) => val.trace(_vis),
            _ => (),
        }
    }
}

unsafe impl<T: Collectable> Finalize for Option<T> {}
impl<T: Collectable> Collectable for Option<T> {}

impl<T: Collectable, H: GcBase> Deref for Gc<T, H> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe {
            let this: Gc<T, H> = H::ReadBarrier::read_barrier::<T>(*self);
            let base = this.base.as_ptr();
            &*(*base).data().cast::<T>()
        }
    }
}

impl<T: Collectable, H: GcBase> DerefMut for Gc<T, H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            let this: Gc<T, H> = H::ReadBarrier::read_barrier::<T>(*self);
            let base = this.base.as_ptr();
            &mut *((*base).data().cast::<T>() as *mut T)
        }
    }
}

unsafe impl<T: Trace> Trace for MaybeUninit<T> {
    fn trace(&mut self, _vis: &mut dyn Visitor) {
        unreachable!()
    }
}

unsafe impl<T> Finalize for MaybeUninit<T> {}

impl<T: Collectable> Collectable for MaybeUninit<T> {
    fn allocation_size(&self) -> usize {
        unreachable!()
    }
}

/*
pub trait WriteBarrier<T> {
    fn write_barrier<H: GcBase>(&self, field: T, heap: &mut H);
}

impl<T: Collectable + ?Sized, U: Collectable + ?Sized> WriteBarrier<Gc<U>> for Gc<T> {
    fn write_barrier<H: GcBase>(&self, _field: Gc<U>, heap: &mut H) {
        heap.write_barrier(*self);
    }
}

impl<T: Collectable + ?Sized, U: Collectable + ?Sized> WriteBarrier<Option<Gc<U>>> for Gc<T> {
    fn write_barrier<H: GcBase>(&self, field: Option<Gc<U>>, heap: &mut H) {
        if let Some(_) = field {
            heap.write_barrier(*self);
        }
    }
}
*/
unsafe impl<T: Trace> Trace for Vec<T> {
    fn trace(&mut self, vis: &mut dyn Visitor) {
        for entry in self.iter_mut() {
            entry.trace(vis);
        }
    }
}

unsafe impl<T: Trace> Trace for Box<T> {
    fn trace(&mut self, _vis: &mut dyn Visitor) {
        (&mut **self).trace(_vis);
    }
}

unsafe impl<T: Trace> Trace for [T] {
    fn trace(&mut self, _vis: &mut dyn Visitor) {
        for x in self.iter_mut() {
            x.trace(_vis);
        }
    }
}

unsafe impl<T: Trace, const N: usize> Trace for [T; N] {
    fn trace(&mut self, _vis: &mut dyn Visitor) {
        for x in self.iter_mut() {
            x.trace(_vis);
        }
    }
}

unsafe impl<T: Trace> Trace for &mut [T] {
    fn trace(&mut self, vis: &mut dyn Visitor) {
        for x in self.iter_mut() {
            x.trace(vis);
        }
    }
}

pub struct WeakInner<H: GcBase> {
    pub value: Option<Gc<dyn Collectable, H>>,
}
/// Weak reference objects, which do not prevent their referents from being made finalizable, finalized, and then reclaimed. Weak references are most often used to implement canonicalizing mappings.
///
///
/// Suppose that the garbage collector determines at a certain point in time that an object is weakly reachable.
/// At that time it will atomically clear all weak references to that object and all weak references to any other weakly-reachable objects from which
/// that object is reachable through a chain of strong and soft references.
/// At the same time it will declare all of the formerly weakly-reachable objects to be finalizable.
/// At the same time or at some later time it will enqueue those newly-cleared weak references that are registered with reference queues.
pub struct Weak<T: Collectable + ?Sized, H: GcBase> {
    value: Gc<WeakInner<H>, H>,
    marker: PhantomData<T>,
}

unsafe impl<H: GcBase> Trace for WeakInner<H> {}
unsafe impl<H: GcBase> Finalize for WeakInner<H> {
    unsafe fn finalize(&mut self) {}
}

impl<H: GcBase> Collectable for WeakInner<H> {}

unsafe impl<T: Collectable + ?Sized, H: GcBase> Trace for Weak<T, H> {
    fn trace(&mut self, vis: &mut dyn Visitor) {
        vis.mark_weak(&mut self.value.base);
    }
}

impl<T: Collectable + ?Sized, H: GcBase> Weak<T, H> {
    pub unsafe fn base(self) -> *mut HeapObjectHeader {
        self.value.base.as_ptr()
    }
    pub unsafe fn set_base(&mut self, hdr: *mut HeapObjectHeader) {
        self.value.base = NonNull::new_unchecked(hdr);
    }
    /// Creates a new weak reference that refers to the given object.
    pub unsafe fn create(mutator: &mut MutatorRef<H>, value: Gc<T, H>) -> Self {
        let stack = mutator.shadow_stack();
        letroot!(value = stack, value);
        let mut inner = mutator.allocate(
            WeakInner { value: None },
            crate::gc_base::AllocationSpace::New,
        );
        inner.value = Some(value.to_dyn());
        mutator.write_barrier(inner.to_dyn());
        Self {
            value: inner,
            marker: PhantomData,
        }
    }
    /// Clears this reference object.
    ///
    ///
    /// This method is invoked only by mutator code; when the garbage collector clears references it does so directly, without invoking this method.
    pub fn clear(mut self) {
        self.value.value = None;
    }
    /// Returns this weak reference object's referent. If this reference object has been cleared, either by the program or by the garbage collector, then this method returns `None`.
    pub fn upgrade(self) -> Option<Gc<T, H>>
    where
        T: Sized,
    {
        self.value.value.map(|x| unsafe { x.downcast_unchecked() })
    }

    /// # NOT FOR USE BY REGULAR CODE, ONLY FOR GC IMPLEMENTATIONS!
    ///
    /// Must be invoked for each weak reference after marking cycle to update weak references.
    pub unsafe fn after_mark(
        &mut self,
        process: impl FnOnce(*mut HeapObjectHeader) -> *mut HeapObjectHeader,
    ) {
        let value = self.value.value;
        match value {
            Some(value) => {
                let new_header = process(value.base.as_ptr());
                if new_header.is_null() {
                    self.value.value = None;
                } else {
                    self.value.value = Some(Gc {
                        base: NonNull::new_unchecked(new_header),
                        marker: PhantomData,
                    });
                }
            }
            _ => (),
        }
    }

    pub fn to_dyn(self) -> Weak<dyn Collectable, H> {
        Weak {
            value: H::ReadBarrier::read_barrier(self.value),
            marker: PhantomData,
        }
    }
}

impl<T: Collectable + ?Sized, H: GcBase> Clone for Weak<T, H> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Collectable + ?Sized, H: GcBase> Copy for Weak<T, H> {}

impl<T: PartialEq + Collectable, H: GcBase> PartialEq for Gc<T, H> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: Eq + Collectable, H: GcBase> Eq for Gc<T, H> {}

impl<T: Hash + Collectable, H: GcBase> Hash for Gc<T, H> {
    fn hash<HS: std::hash::Hasher>(&self, state: &mut HS) {
        (**self).hash(state);
    }
}

impl<T: std::fmt::Debug + Collectable, H: GcBase> std::fmt::Debug for Gc<T, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", **self)
    }
}

impl<T: std::fmt::Display + Collectable, H: GcBase> std::fmt::Display for Gc<T, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", **self)
    }
}

impl<T: std::cmp::PartialOrd + Collectable, H: GcBase> std::cmp::PartialOrd for Gc<T, H> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        (**self).partial_cmp(&**other)
    }
}

impl<T: std::cmp::Ord + Collectable, H: GcBase> std::cmp::Ord for Gc<T, H> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (**self).cmp(&**other)
    }
}

unsafe impl Trace for std::str::Bytes<'_> {}
