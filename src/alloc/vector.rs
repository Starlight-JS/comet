use std::{marker::PhantomData, mem::size_of};

use crate::{
    api::{Collectable, Finalize, Gc, Handle, HandleMut, Trace},
    base::GcBase,
};

/// Vector for heap allocated values.
#[repr(C)]
pub struct Vector<T: Collectable + Sized, H: GcBase = crate::Heap> {
    marker: PhantomData<H>,
    size: u32,
    capacity: u32,
    data_start: [T; 0],
}

impl<T: Collectable + Sized, H: 'static + GcBase> Vector<T, H> {
    #[inline]
    pub fn new(heap: &mut H, capacity: usize) -> Gc<Self> {
        let init = Self {
            marker: PhantomData,
            size: 0,
            capacity: capacity as _,
            data_start: [],
        };
        heap.allocate(init)
    }

    pub fn begin(&self) -> *const T {
        self.data_start.as_ptr()
    }

    pub fn end(&self) -> *const T {
        unsafe { self.data_start.as_ptr().add(self.size as _) }
    }

    pub fn begin_mut(&mut self) -> *mut T {
        self.data_start.as_mut_ptr()
    }

    pub fn end_mut(&mut self) -> *mut T {
        unsafe { self.data_start.as_mut_ptr().add(self.size as _) }
    }

    pub fn len(&self) -> usize {
        self.size as _
    }

    pub fn capacity(&self) -> usize {
        self.capacity as _
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.size as usize {
            None
        } else {
            Some(unsafe { &*self.begin().add(index) })
        }
    }

    pub unsafe fn get_unchecked(&self, index: usize) -> &T {
        self.get(index).unwrap_unchecked()
    }

    /// Get mutable reference to GCed pointer. When updating be sure to use write-barrier (when your GC is generational)
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.size as usize {
            None
        } else {
            Some(unsafe { &mut *self.begin_mut().add(index) })
        }
    }

    pub unsafe fn get_unchecked_mut(&mut self, index: usize) -> &mut T {
        self.get_mut(index).unwrap_unchecked()
    }
}

impl<T: Collectable + Sized, H: 'static + GcBase> Vector<Gc<T>, H> {
    /// Returns immutable handle to the element at `index`.
    ///
    ///
    ///  # Safety
    /// Safe because to get access to Vector you have to have Vector rooted.
    ///
    pub fn at<'a>(&self, index: usize) -> Option<Handle<'a, T>> {
        if index >= self.size as usize {
            None
        } else {
            Some(unsafe { (*self.begin().add(index)).fake_handle() })
        }
    }
}
impl<'a, T: Collectable + ?Sized, H: 'static + GcBase> HandleMut<'a, Vector<Gc<T>, H>> {
    /// Returns mutable handle to the element at `index`.
    ///
    /// # Safety
    /// Safe because to get HandleMut you have to have Vector rooted.
    ///
    pub fn at_mut(&mut self, index: usize) -> Option<HandleMut<'a, T>> {
        if index >= self.size as usize {
            None
        } else {
            Some(unsafe { (*self.begin_mut().add(index)).fake_handle_mut() })
        }
    }
}

impl<'a, T: Collectable + Sized, H: 'static + GcBase> HandleMut<'a, Vector<T, H>> {
    fn grow(&mut self, heap: &mut H, capacity: usize) {
        let old_capacity = self.capacity();
        let new_capacity = capacity;
        if new_capacity == old_capacity {
            return;
        }

        let len = self.len();
        let mut new_vec = Vector::<T, H>::new(heap, new_capacity);
        unsafe {
            std::ptr::copy_nonoverlapping(self.begin(), new_vec.fake_handle_mut().begin_mut(), len);
            new_vec.fake_handle_mut().size = len as _;
        }
        self.write(new_vec);
    }

    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        let len = self.len();
        if len == 0 {
            return None;
        }

        unsafe {
            let v = self.begin().add(len - 1).read();
            self.size -= 1;
            Some(v)
        }
    }

    #[inline]
    pub fn push(&mut self, heap: &mut H, value: T) -> &mut T
    where
        T: Unpin,
    {
        let (len, cap) = (self.len(), self.capacity());
        if len == cap {
            return self.push_slow(heap, value);
        }

        let dst = unsafe { self.begin_mut().add(len) };
        unsafe {
            dst.write(value);
        }
        self.size += 1;
        unsafe { &mut *dst }
    }
    #[cold]
    fn push_slow(&mut self, heap: &mut H, value: T) -> &mut T
    where
        T: Unpin,
    {
        let stack = heap.shadow_stack();
        letroot!(value = stack, Some(value));

        self.grow(heap, next_capacity::<T>(self.capacity()));
        let dst = unsafe { self.begin_mut().add(self.len()) };
        unsafe {
            dst.write(value.take().unwrap_unchecked());
        }
        self.size += 1;
        unsafe { &mut *dst }
    }

    pub fn remove(&mut self, index: usize) -> Option<T> {
        let len = self.len();
        if index >= len {
            return None;
        }

        unsafe {
            let p = self.begin_mut().add(index);
            let x = p.read();
            let src = p.add(1);
            let dst = p;
            let count = len - index - 1;
            std::ptr::copy(src, dst, count);
            self.size = len as u32 - 1;
            Some(x)
        }
    }

    pub fn remove_item<V>(&mut self, item: &V) -> Option<T>
    where
        T: PartialEq<V>,
    {
        let len = self.len();
        for i in 0..len {
            unsafe {
                if *self.get_unchecked(i) == *item {
                    return self.remove(i);
                }
            }
        }
        None
    }

    pub fn reserve(&mut self, heap: &mut H, additional: usize) {
        let capacity = self.capacity();
        let total_required = self.len() + additional;

        if total_required <= capacity {
            return;
        }

        let mut new_capacity = next_capacity::<T>(capacity);
        while new_capacity < total_required {
            new_capacity = next_capacity::<T>(new_capacity);
        }

        self.grow(heap, new_capacity);
    }

    pub fn reserve_exact(&mut self, heap: &mut H, additional: usize) {
        let capacity = self.capacity();
        let len = self.len();

        let total_required = len + additional;
        if capacity >= total_required {
            return;
        }

        self.grow(heap, total_required);
    }

    pub fn truncate(&mut self, len: usize) {
        let self_len = self.len();

        if len >= self_len {
            return;
        }

        self.size = len as _;

        if !core::mem::needs_drop::<T>() {
            return;
        }

        let s =
            unsafe { core::slice::from_raw_parts_mut(self.begin_mut().add(len), self_len - len) };

        unsafe {
            core::ptr::drop_in_place(s);
        }
    }
}
impl<T: Collectable + Sized, H: 'static + GcBase> Collectable for Vector<T, H> {
    fn allocation_size(&self) -> usize {
        size_of::<Self>() + (self.capacity as usize * size_of::<Gc<T>>())
    }
}

unsafe impl<T: Collectable + Sized, H: 'static + GcBase> Trace for Vector<T, H> {
    fn trace(&mut self, _vis: &mut dyn crate::api::Visitor) {
        unsafe {
            let mut cursor = self.begin_mut();
            let end = self.end_mut();
            while cursor < end {
                (*cursor).trace(_vis);
                cursor = cursor.add(1);
            }
        }
    }
}

unsafe impl<T: Collectable + Sized, H: GcBase> Finalize for Vector<T, H> {}

const fn next_capacity<T>(capacity: usize) -> usize {
    let elem_size = core::mem::size_of::<T>();

    if capacity == 0 {
        return match elem_size {
            1 => 8,
            2..=1024 => 4,
            _ => 1,
        };
    }

    2 * capacity
}
