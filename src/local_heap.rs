use atomic::Atomic;

/// LocalHeap is used by the GC to track all threads with heap access in order to
/// stop them before performing a collection. LocalHeaps can be either Parked or
/// Running and are in Parked mode when initialized.
///   Running: Thread is allowed to access the heap but needs to give the GC the
///            chance to run regularly by manually invoking Safepoint(). The
///            thread can be parked using ParkedScope.
///   Parked:  Heap access is not allowed, so the GC will not stop this thread
///            for a collection. Useful when threads do not need heap access for
///            some time or for blocking operations like locking a mutex.
pub struct LocalHeap {
    state: Atomic<ThreadState>,
    prev: *mut LocalHeap,
    next: *mut LocalHeap,
    is_main: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ThreadState {
    /// Threads in this state are allowed to access the heap.
    Running,
    /// Thread was parked, which means that the thread is not allowed to access
    /// or manipulate the heap in any way. This is considered to be a safepoint.
    Parked,

    /// SafepointRequested is used for Running threads to force Safepoint() and
    /// Park() into the slow path.
    SafepointRequested,
    /// A thread transitions into this state from SafepointRequested when it
    /// enters a safepoint.
    Safepoint,
    /// This state is used for Parked background threads and forces Unpark() into
    /// the slow path. It prevents Unpark() to succeed before the safepoint
    /// operation is finished.
    ParkedSafepointRequested,
}

impl LocalHeap {
    pub fn safepoint(&self) {}
}
