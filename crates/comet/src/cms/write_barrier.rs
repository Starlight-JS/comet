//! Retreating wavefront write barrier implementation for CMS.

use super::marker::Marker;
use crate::api::{HeapObjectHeader, GC_BLACK, GC_GREY};

#[inline(always)]
pub(super) unsafe fn write_barrier_impl(marker: &Marker, object: *mut HeapObjectHeader) {
    // if object color is black it was already visited, we set its color to grey and push
    // it to write barrier worklist so concurrent marker will eventually process it in
    // concurrent marking cycle or at final marking cycle.
    if (*object).set_color(GC_BLACK, GC_GREY) {
        write_barrier_slow(marker, object);
    }
}
#[cold]
unsafe fn write_barrier_slow(marker: &Marker, object: *mut HeapObjectHeader) {
    marker
        .marking_worklists()
        .write_barrier_worklist()
        .push(object as usize);
}
