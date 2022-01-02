use std::{
    hint::unreachable_unchecked,
    marker::PhantomData,
    mem::{size_of, MaybeUninit},
    ops::{Deref, DerefMut},
    ptr::{null_mut, NonNull},
    sync::atomic::AtomicU16,
};

use crate::{
    gc_base::GcBase, large_space::PreciseAllocation, mutator::MutatorRef, small_type_id, utils::*,
};
use atomic::Ordering;
use mopa::mopafy;
pub unsafe trait Trace {
    fn trace(&mut self, _vis: &mut dyn Visitor) {}
}

pub trait Collectable: Trace + Finalize + mopa::Any {
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

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HeapObjectHeader {
    pub value: u64,

    pub padding: u16,
    pub padding2: u16,
    pub type_id: u32,
}

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

/// A type that should be used to store GCed struct fields. It is not movable but dereferencable.
#[repr(transparent)]
pub struct Field<T: Collectable + ?Sized> {
    base: Gc<T>,
}
impl<T: Collectable + ?Sized> Field<T> {
    pub fn as_dyn(&self) -> &dyn Collectable {
        unsafe {
            let base = self.base.base.as_ptr();
            let trait_object = mopa::TraitObject {
                data: (*base).data() as *mut (),
                vtable: (*base).vtable() as *mut (),
            };

            std::mem::transmute(trait_object)
        }
    }
    pub fn as_dyn_mut(&mut self) -> &mut dyn Collectable {
        unsafe {
            let base = self.base.base.as_ptr();
            let trait_object = mopa::TraitObject {
                data: (*base).data() as *mut (),
                vtable: (*base).vtable() as *mut (),
            };

            std::mem::transmute(trait_object)
        }
    }

    pub fn is<U: Collectable>(&self) -> bool {
        self.base.is::<U>()
    }

    pub fn downcast<U: Collectable>(&self) -> Option<Gc<U>> {
        self.base.downcast::<U>()
    }

    pub fn to_dyn(&self) -> Gc<dyn Collectable> {
        self.base.to_dyn()
    }

    pub fn to_gc(&self) -> Gc<T> {
        self.base
    }
}
impl<T: Collectable + Sized> Deref for Field<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe {
            let base = self.base.base;
            let data = (*base.as_ptr()).data().cast::<T>();
            &*data
        }
    }
}

impl<T: Collectable + Sized> DerefMut for Field<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            let base = self.base.base;
            let data = (*base.as_ptr()).data().cast::<T>() as *mut T;

            &mut *data
        }
    }
}

unsafe impl<T: Collectable + ?Sized> Trace for Field<T> {
    fn trace(&mut self, vis: &mut dyn Visitor) {
        vis.mark_object(&mut self.base.base);
    }
}

unsafe impl<T: Collectable + ?Sized> Finalize for Field<T> {}
unsafe impl<T: Collectable + ?Sized> Finalize for Gc<T> {}
impl<T: Collectable + ?Sized> Collectable for Gc<T> {}
pub(crate) fn vtable_of<T: Collectable>() -> usize {
    let x = null_mut::<T>();
    unsafe { std::mem::transmute::<_, mopa::TraitObject>(x as *mut dyn Collectable).vtable as _ }
}

#[allow(dead_code)]
pub(crate) fn vtable_of_trace<T: Trace>() -> usize {
    let x = null_mut::<T>();
    unsafe { std::mem::transmute::<_, mopa::TraitObject>(x as *mut dyn Trace).vtable as _ }
}

pub struct Gc<T: Collectable + ?Sized> {
    pub(crate) base: NonNull<HeapObjectHeader>,
    pub(crate) marker: PhantomData<T>,
}
impl<T: Collectable + Sized> Gc<MaybeUninit<T>> {
    pub unsafe fn assume_init(self) -> Gc<T> {
        Gc {
            base: self.base,
            marker: Default::default(),
        }
    }
}
impl<T: Collectable + ?Sized> Gc<T> {
    pub fn to_field(self) -> Field<T> {
        Field { base: self }
    }
    #[inline]
    pub fn to_dyn(self) -> Gc<dyn Collectable> {
        Gc {
            base: self.base,
            marker: PhantomData,
        }
    }

    #[inline(always)]
    pub fn is<U: Collectable>(&self) -> bool {
        unsafe { (*self.base.as_ptr()).type_id == small_type_id::<U>() }
    }
    #[inline]
    pub fn vtable(&self) -> usize {
        unsafe { (*self.base.as_ptr()).vtable() }
    }
    #[inline]
    pub fn downcast<U: Collectable>(&self) -> Option<Gc<U>> {
        if self.is::<U>() {
            Some(Gc {
                base: self.base,
                marker: PhantomData,
            })
        } else {
            None
        }
    }
    #[inline]
    pub unsafe fn downcast_unchecked<U: Collectable>(&self) -> Gc<U> {
        self.downcast().unwrap_or_else(|| unreachable_unchecked())
    }

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

impl<T: Collectable + ?Sized> Clone for Gc<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Collectable + ?Sized> Copy for Gc<T> {}

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
unsafe impl<T: Collectable + ?Sized> Trace for Gc<T> {
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

impl<T: Collectable + ?Sized> std::fmt::Pointer for Gc<T> {
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

impl<T: Collectable> Collectable for Field<T> {}

impl<T: Collectable> Deref for Gc<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe {
            let base = self.base.as_ptr();
            &*(*base).data().cast::<T>()
        }
    }
}

impl<T: Collectable> DerefMut for Gc<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            let base = self.base.as_ptr();
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

pub struct WeakInner {
    pub value: Option<Gc<dyn Collectable>>,
}

pub struct Weak<T: Collectable + ?Sized> {
    value: Gc<WeakInner>,
    marker: PhantomData<T>,
}

unsafe impl Trace for WeakInner {}
unsafe impl Finalize for WeakInner {
    unsafe fn finalize(&mut self) {}
}

impl Collectable for WeakInner {}

unsafe impl<T: Collectable + ?Sized> Trace for Weak<T> {
    fn trace(&mut self, vis: &mut dyn Visitor) {
        vis.mark_weak(&mut self.value.base);
    }
}

impl<T: Collectable + ?Sized> Weak<T> {
    pub unsafe fn base(self) -> *mut HeapObjectHeader {
        self.value.base.as_ptr()
    }
    pub unsafe fn set_base(&mut self, hdr: *mut HeapObjectHeader) {
        self.value.base = NonNull::new_unchecked(hdr);
    }
    pub fn create(mutator: &mut MutatorRef<impl GcBase>, value: Gc<T>) -> Self {
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
    pub fn upgrade(self) -> Option<Gc<T>>
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

    pub fn to_dyn(self) -> Weak<dyn Collectable> {
        Weak {
            value: self.value,
            marker: PhantomData,
        }
    }
}

impl<T: Collectable + ?Sized> Clone for Weak<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Collectable + ?Sized> Copy for Weak<T> {}
