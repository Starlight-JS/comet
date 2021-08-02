use std::ptr::null_mut;

use crate::{gc_info_table::GC_TABLE, header::HeapObjectHeader};
use parking_lot::{lock_api::RawMutex, RawMutex as Mutex};

/// Precise allocation used for large objects (>= LARGE_CUTOFF).
/// Starlight uses mimalloc that already knows what to do for large allocations. The GC shouldn't
/// have to think about such things. That's where PreciseAllocation comes in. We will allocate large
/// objects directly using mi_malloc, and put the PreciseAllocation header just before them. We can detect
/// when a *mut GcPointerBase is a PreciseAllocation because it will have the ATOM_SIZE / 2 bit set.
#[repr(C)]
pub struct PreciseAllocation {
    //pub link: LinkedListLink,
    /// allocation request size
    pub cell_size: usize,

    /// index in precise_allocations
    pub index_in_space: u32,
    /// Is alignment adjusted?
    //pub is_newly_allocated: bool,
    pub adjusted_alignment: bool,
    /// Is this even valid allocation?
    pub has_valid_cell: bool,
}

impl PreciseAllocation {
    /// Alignment of allocation.
    pub const ALIGNMENT: usize = 16;
    /// Alignment of pointer returned by `Self::cell`.
    pub const HALF_ALIGNMENT: usize = Self::ALIGNMENT / 2;
    /// Check if raw_ptr is precisely allocated.
    pub fn is_precise(raw_ptr: *mut ()) -> bool {
        (raw_ptr as usize & Self::HALF_ALIGNMENT) != 0
    }
    /// Create PreciseAllocation from pointer
    pub fn from_cell(ptr: *mut HeapObjectHeader) -> *mut Self {
        unsafe {
            ptr.cast::<u8>()
                .offset(-(Self::header_size() as isize))
                .cast()
        }
    }
    /// Return base pointer
    #[inline]
    pub fn base_pointer(&self) -> *mut () {
        if self.adjusted_alignment {
            ((self as *const Self as isize) - (Self::HALF_ALIGNMENT as isize)) as *mut ()
        } else {
            self as *const Self as *mut ()
        }
    }

    /// Return cell address, it is always aligned to `Self::HALF_ALIGNMENT`.
    pub fn cell(&self) -> *mut HeapObjectHeader {
        let addr = unsafe { (self as *const Self as *const u8).add(Self::header_size()) };
        addr as _
    }
    /// Return true if raw_ptr is above lower bound
    pub fn above_lower_bound(&self, raw_ptr: *mut ()) -> bool {
        let ptr = raw_ptr;
        let begin = self.cell() as *mut ();
        ptr >= begin
    }
    /// Return true if raw_ptr below upper bound
    pub fn below_upper_bound(&self, raw_ptr: *mut ()) -> bool {
        let ptr = raw_ptr;
        let begin = self.cell() as *mut ();
        let end = (begin as usize + self.cell_size) as *mut ();
        ptr <= (end as usize + 8) as *mut ()
    }
    /// Returns header size + required alignment to make cell be aligned to 8.
    pub const fn header_size() -> usize {
        ((core::mem::size_of::<PreciseAllocation>() + Self::HALF_ALIGNMENT - 1)
            & !(Self::HALF_ALIGNMENT - 1))
            | Self::HALF_ALIGNMENT
    }
    /// Does this allocation contains raw_ptr?
    pub fn contains(&self, raw_ptr: *mut ()) -> bool {
        self.above_lower_bound(raw_ptr) && self.below_upper_bound(raw_ptr)
    }

    /// Finalize cell if this allocation is not marked.
    pub fn sweep(&mut self) -> bool {
        let cell = self.cell();
        unsafe {
            if (*cell).set_state(
                crate::header::CellState::PossiblyBlack,
                crate::header::CellState::DefinitelyWhite,
            ) {
            } else {
                self.has_valid_cell = false;

                let info = GC_TABLE.get_gc_info((*cell).get_gc_info_index());
                if let Some(cb) = info.finalize {
                    cb((*cell).payload() as _);
                }

                return false;
            }
        }
        true
    }
    /// Try to create precise allocation (no way that it will return null for now).
    pub fn try_create(size: usize, index_in_space: u32) -> *mut Self {
        let adjusted_alignment_allocation_size = Self::header_size() + size + Self::HALF_ALIGNMENT;
        unsafe {
            let mut space = libc::malloc(adjusted_alignment_allocation_size).cast::<u8>();

            //let mut space = libc::malloc(adjusted_alignment_allocation_size);
            let mut adjusted_alignment = false;
            if !is_aligned_for_precise_allocation(space) {
                space = space.add(Self::HALF_ALIGNMENT);
                adjusted_alignment = true;
                assert!(is_aligned_for_precise_allocation(space));
            }
            assert!(size != 0);
            space.cast::<Self>().write(Self {
                //link: LinkedListLink::new(),
                adjusted_alignment,

                //is_newly_allocated: true,
                has_valid_cell: true,
                cell_size: size,
                index_in_space,
            });

            space.cast()
        }
    }
    /// return cell size
    pub fn cell_size(&self) -> usize {
        self.cell_size
    }
    /// Destroy this allocation
    pub fn destroy(&mut self) {
        let base = self.base_pointer();
        unsafe {
            libc::free(base as _);
        }
    }
}
/// Check if `mem` is aligned for precise allocation
pub fn is_aligned_for_precise_allocation(mem: *mut u8) -> bool {
    let allocable_ptr = mem as usize;
    (allocable_ptr & (PreciseAllocation::ALIGNMENT - 1)) == 0
}
/// This space contains objects which are larger than the size limits of other spaces.
/// Each object gets its own malloc'd region of memory.
/// Large objects are never moved by the garbage collector.
pub struct LargeObjectSpace {
    pub(crate) large_space_mutex: Mutex,
    pub(crate) allocations: Vec<*mut PreciseAllocation>,
}

impl LargeObjectSpace {
    pub(crate) fn new() -> Self {
        Self {
            allocations: Vec::new(),
            large_space_mutex: Mutex::INIT,
        }
    }
    #[inline]
    pub fn contains(&self, pointer: *const u8) -> *mut HeapObjectHeader {
        unsafe {
            if self.allocations.is_empty() {
                return null_mut();
            }
            if (**self.allocations.first().unwrap()).above_lower_bound(pointer as _)
                && (**self.allocations.last().unwrap()).below_upper_bound(pointer as _)
            {
                let prec = PreciseAllocation::from_cell(pointer as _);
                match self.allocations.binary_search_by(|a| a.cmp(&prec)) {
                    Ok(ix) => (*self.allocations[ix]).cell(),
                    _ => null_mut(),
                }
            } else {
                null_mut()
            }
        }
    }
    pub fn sweep(&mut self) -> usize {
        let mut alive = 0;
        self.allocations.retain(|allocation| unsafe {
            let allocation = &mut **allocation;
            if !allocation.sweep() {
                allocation.destroy();
                false
            } else {
                alive += allocation.cell_size();
                true
            }
        });
        alive
    }
    /// Sort allocations before consrvative scan.
    pub fn prepare_for_conservative_scan(&mut self) {
        self.allocations.sort_by(|a, b| a.cmp(b));
        for (index, alloc) in self.allocations.iter().enumerate() {
            unsafe {
                (**alloc).index_in_space = index as _;
            }
        }
    }
    /// Obtains reference to all precise allocations. Used by conservative scan.
    pub(crate) fn allocations(&self) -> &[*mut PreciseAllocation] {
        &self.allocations
    }
    pub fn allocate(&mut self, size: usize) -> *mut HeapObjectHeader {
        unsafe {
            self.large_space_mutex.lock();
            let index = self.allocations.len();
            let memory = PreciseAllocation::try_create(size, index as _);
            if memory.is_null() {
                panic!("LargeObjectSpace: OOM");
            }

            self.allocations.push(memory);
            self.large_space_mutex.unlock();
            let cell = (*memory).cell();
            (*cell).set_size(0); // size of 0 means object is large.
            (*memory).cell()
        }
    }
}
