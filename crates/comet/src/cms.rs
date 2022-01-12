//! # Concurrent Mark-and-Sweep
//!
//! Simple CMS collector. Heap is divided into 128KB pages, small objects (<=64KB) are allocated
//! into normal pages which contain free-list. Large objects go to large pages and large page contains only
//! one object.
//!
//!
//! ## GC cycle
//!
//! GC cycle is triggered when certain amount of memory is allocated and it starts with Initial Marking.
//!
//! ## Initial marking
//! Initial marking visits mutators roots, executes mark constraints, after all roots are scanned
//! we start concurrent marker thread and resume the mutators.
//! ## Concurrent marking
//! Concurrent marking happens in background thread and it simply processes objects that are in grey set by
//! colouring them to black. Note that mutator might store reference to black object so write barriers are used
//! to ensure that we will revisit black object again. Write barrier algorithm looks like this:
//! ```python
//! def write_barrier(marker, object):
//!     if marker.is_marking() && object.color == BLACK:
//!         object.color = GREY
//!         marker.worklist.push(object)
//! ```
//!
//! When marking worklist is empty it stops and executes final marking cycle.
//!
//! ## Final marking
//!
//! Final marking is executed in STW pause and in final marking phase we re-mark roots, process weak refs, execute finalizers
//! and setup concurrent sweeper with currently allocated heap pages.
//!
//! ## Concurrent sweeping
//!
//! Sweeper works by draining heap pages and sweeping each object in them, after page is swept
//! it is added to global page free-list so mutators can again allocate into them.
//!
//!
//! # What if there is no enough memory and GC is running?
//!
//! If there is no enough memory to allocate object and GC is running, GC cycle "degrades" to Full STW cycle.
//! In this cycle we stop all mutators and execute all stages of GC in STW pause.
//!
//! # How GC decides when there is no enough memory?
//! It does not, at the start of cycle we set GC threshold to 35% of current GC heap and when it is reached
//! we simply switch to Degraded GC. At the end of the sweeping threshold is updated to be current_heap_size + 50% of current heap size.
//! This allows us to perform concurrent cycles more often without going to degraded cycles.

pub mod block;
pub mod marker;
pub mod marking_worklist;
pub mod space;
pub mod write_barrier;

/// Concurrent Mark&Sweep heap.
///
///
///
/// `CONCURRENT` const determines if GC does concurrent marking&sweeping or always performs collection in STW, useful for debugging.
pub struct ConcurrentMarkSweep<const CONCURRENT: bool = true> {}
