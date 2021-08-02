use std::mem::swap;

use crate::{
    block::Block,
    gc_info_table::GC_TABLE,
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
}
#[inline]
unsafe fn trace_desc(ptr: *mut HeapObjectHeader) -> TraceDescriptor {
    TraceDescriptor {
        base_object_payload: (*ptr).payload(),
        callback: GC_TABLE.get_gc_info((*ptr).get_gc_info_index()).trace,
    }
}
impl VisitorTrait for MarkingVisitor {
    fn visit(
        &mut self,
        this: *const u8,
        descriptor: crate::internal::trace_trait::TraceDescriptor,
    ) {
        unsafe {
            let header = HeapObjectHeader::from_object(this);

            if (*header).set_state(CellState::DefinitelyWhite, CellState::PossiblyGrey) {
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
                    let block = Block::get_block_ptr(pointer);
                    if (*block).is_in_block(pointer) {
                        let cell = (*block).cell_from_ptr(pointer);
                        if (*self.heap).live_bitmap.test(cell) {
                            let hdr = cell.cast::<HeapObjectHeader>();
                            println!("Small object {:p} at {:p}", hdr, scan);
                            self.visit((*hdr).payload(), trace_desc(hdr));
                        }
                    }
                } else {
                    let hdr = (*self.heap).large_space.contains(pointer);

                    if !hdr.is_null() {
                        println!("Large object {:p} at {:p}", hdr, scan);
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
    pub fn run(&mut self) {
        let mut vis = MarkingVisitor {
            worklist: vec![],
            heap: self.heap.global.get(),
        };

        self.heap.safepoint().iterate(|local| unsafe {
            let mut from = (*local).bounds.origin;
            let mut to = (*local).last_sp.get();
            if to.is_null() {
                return;
            }
            if from > to {
                swap(&mut to, &mut from);
            }

            vis.visit_conservative(from.cast(), to.cast());
        });

        while let Some(desc) = vis.worklist.pop() {
            unsafe {
                let hdr = HeapObjectHeader::from_object(desc.base_object_payload);
                println!("Blacken {:p}", hdr);
                assert!((*hdr).set_state(CellState::PossiblyGrey, CellState::PossiblyBlack));
                (desc.callback)(&mut Visitor { vis: &mut vis }, desc.base_object_payload);
            }
        }
    }
}
