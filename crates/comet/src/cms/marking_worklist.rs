use crossbeam::queue::SegQueue;

pub struct MarkingWorklists {
    marking_worklists: SegQueue<usize>,
    write_barrier_worklist: SegQueue<usize>,
}

impl MarkingWorklists {
    pub fn write_barrier_worklist(&self) -> &SegQueue<usize> {
        &self.write_barrier_worklist
    }
}
