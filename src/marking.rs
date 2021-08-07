use crate::{
    gc_info_table::GC_TABLE,
    gc_size,
    global_allocator::GlobalAllocator,
    header::CellState,
    header::HeapObjectHeader,
    heap::Heap,
    internal::trace_trait::TraceDescriptor,
    visitor::{Visitor, VisitorTrait},
};

pub struct MarkingVisitor {
    worklist: Vec<TraceDescriptor>,
    heap: *mut GlobalAllocator,
    h: *mut Heap,
    bytes_visited: usize,
}
#[inline]
unsafe fn trace_desc(ptr: *mut HeapObjectHeader) -> TraceDescriptor {
    TraceDescriptor {
        base_object_payload: (*ptr).payload(),
        callback: GC_TABLE.get_gc_info((*ptr).get_gc_info_index()).trace,
    }
}
impl VisitorTrait for MarkingVisitor {
    fn heap(&self) -> *mut Heap {
        self.h
    }
    fn visit(
        &mut self,
        this: *const u8,
        descriptor: crate::internal::trace_trait::TraceDescriptor,
    ) {
        unsafe {
            let header = HeapObjectHeader::from_object(this);
            let res = (*self.h).test_and_set_marked(header);

            if !res {
                (*header).force_set_state(CellState::PossiblyGrey);
                self.bytes_visited += gc_size(header);
                self.worklist.push(descriptor);
            }
        }
    }

    fn visit_conservative(&mut self, from: *const *const u8, to: *const *const u8) {
        let mut scan = from;
        let end = to;

        while scan < end {
            unsafe {
                let pointer = scan.read();

                if (*self.heap).block_allocator.is_in_space(pointer) {
                    let cell = pointer;
                    if (*self.heap).live_bitmap.test(cell) {
                        let hdr = cell.cast::<HeapObjectHeader>();

                        self.visit((*hdr).payload(), trace_desc(hdr as _));
                    }
                } else {
                    let hdr = (*self.heap).large_space.contains(pointer);

                    if !hdr.is_null() {
                        self.visit((*hdr).payload(), trace_desc(hdr));
                    }
                }
                scan = scan.add(1);
            }
        }
    }
}

pub struct SynchronousMarking<'a> {
    heap: &'a mut Heap,
}

impl<'a> SynchronousMarking<'a> {
    pub fn new(heap: &'a mut Heap) -> Self {
        Self { heap }
    }
    pub fn run(&mut self) -> usize {
        let mut vis = MarkingVisitor {
            worklist: vec![],
            heap: &mut self.heap.global,
            bytes_visited: 0,
            h: self.heap as *mut _,
        };

        let mut constraints = std::mem::replace(&mut self.heap.constraints, vec![]);
        for c in constraints.iter_mut() {
            c.execute(&mut Visitor { vis: &mut vis })
        }
        self.heap.constraints = constraints;
        while let Some(desc) = vis.worklist.pop() {
            unsafe {
                let hdr = HeapObjectHeader::from_object(desc.base_object_payload);

                assert!((*hdr).set_state(CellState::PossiblyGrey, CellState::PossiblyBlack));
                (desc.callback)(&mut Visitor { vis: &mut vis }, desc.base_object_payload);
            }
        }
        vis.bytes_visited
    }
}
