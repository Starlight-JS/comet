use std::collections::VecDeque;
use std::ptr;
/// Trait that provides `retain_mut` method.
pub trait RetainMut<T> {
    /// Retains only the elements specified by the predicate, passing a mutable reference to it.
    ///
    /// In other words, remove all elements `e` such that `f(&mut e)` returns `false`.
    /// This method operates in place, visiting each element exactly once in the
    /// original order, and preserves the order of the retained elements.
    fn retain_mut<F>(&mut self, f: F)
    where
        F: FnMut(&mut T) -> bool;
}

impl<T> RetainMut<T> for Vec<T> {
    // The implementation is based on
    // https://github.com/rust-lang/rust/blob/3e21768a0a3fc84befd1cbe825ae6849e9941b73/library/alloc/src/vec/mod.rs#L1475-L1569
    fn retain_mut<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut T) -> bool,
    {
        let original_len = self.len();
        // Avoid double drop if the drop guard is not executed,
        // since we may make some holes during the process.
        unsafe { self.set_len(0) };

        // Vec: [Kept, Kept, Hole, Hole, Hole, Hole, Unchecked, Unchecked]
        //      |<-              processed len   ->| ^- next to check
        //                  |<-  deleted cnt     ->|
        //      |<-              original_len                          ->|
        // Kept: Elements which predicate returns true on.
        // Hole: Moved or dropped element slot.
        // Unchecked: Unchecked valid elements.
        //
        // This drop guard will be invoked when predicate or `drop` of element panicked.
        // It shifts unchecked elements to cover holes and `set_len` to the correct length.
        // In cases when predicate and `drop` never panick, it will be optimized out.
        struct BackshiftOnDrop<'a, T> {
            v: &'a mut Vec<T>,
            processed_len: usize,
            deleted_cnt: usize,
            original_len: usize,
        }

        impl<T> Drop for BackshiftOnDrop<'_, T> {
            fn drop(&mut self) {
                if self.deleted_cnt > 0 {
                    // SAFETY: Trailing unchecked items must be valid since we never touch them.
                    unsafe {
                        ptr::copy(
                            self.v.as_ptr().add(self.processed_len),
                            self.v
                                .as_mut_ptr()
                                .add(self.processed_len - self.deleted_cnt),
                            self.original_len - self.processed_len,
                        );
                    }
                }
                // SAFETY: After filling holes, all items are in contiguous memory.
                unsafe {
                    self.v.set_len(self.original_len - self.deleted_cnt);
                }
            }
        }

        let mut g = BackshiftOnDrop {
            v: self,
            processed_len: 0,
            deleted_cnt: 0,
            original_len,
        };

        // process_one return a bool indicates whether the processing element should be retained.
        #[inline(always)]
        fn process_one<F, T, const DELETED: bool>(f: &mut F, g: &mut BackshiftOnDrop<'_, T>) -> bool
        where
            F: FnMut(&mut T) -> bool,
        {
            // SAFETY: Unchecked element must be valid.
            let cur = unsafe { &mut *g.v.as_mut_ptr().add(g.processed_len) };
            if !f(cur) {
                // Advance early to avoid double drop if `drop_in_place` panicked.
                g.processed_len += 1;
                g.deleted_cnt += 1;
                // SAFETY: We never touch this element again after dropped.
                unsafe { ptr::drop_in_place(cur) };
                // We already advanced the counter.
                return false;
            }
            if DELETED {
                // SAFETY: `deleted_cnt` > 0, so the hole slot must not overlap with current element.
                // We use copy for move, and never touch this element again.
                unsafe {
                    let hole_slot = g.v.as_mut_ptr().add(g.processed_len - g.deleted_cnt);
                    ptr::copy_nonoverlapping(cur, hole_slot, 1);
                }
            }
            g.processed_len += 1;
            return true;
        }

        // Stage 1: Nothing was deleted.
        while g.processed_len != original_len {
            if !process_one::<F, T, false>(&mut f, &mut g) {
                break;
            }
        }

        // Stage 2: Some elements were deleted.
        while g.processed_len != original_len {
            process_one::<F, T, true>(&mut f, &mut g);
        }

        // All item are processed. This can be optimized to `set_len` by LLVM.
        drop(g);
    }
}

impl<T> RetainMut<T> for VecDeque<T> {
    // The implementation is based on
    // https://github.com/rust-lang/rust/blob/3e21768a0a3fc84befd1cbe825ae6849e9941b73/library/alloc/src/collections/vec_deque/mod.rs#L2148-L2180
    fn retain_mut<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut T) -> bool,
    {
        let len = self.len();
        let mut idx = 0;
        let mut cur = 0;

        // Stage 1: All values are retained.
        while cur < len {
            if !f(&mut self[cur]) {
                cur += 1;
                break;
            }
            cur += 1;
            idx += 1;
        }
        // Stage 2: Swap retained value into current idx.
        while cur < len {
            if !f(&mut self[cur]) {
                cur += 1;
                continue;
            }

            self.swap(idx, cur);
            cur += 1;
            idx += 1;
        }
        // Stage 3: Trancate all values after idx.
        if cur != idx {
            self.truncate(idx);
        }
    }
}
