use parking_lot::Mutex;

use crate::visitor::Visitor;

pub trait MarkingTask {
    fn execute(&mut self, visitor: &mut Visitor);
}

pub struct TaskScheduler(pub(crate) Mutex<Vec<Box<dyn MarkingTask>>>);
impl TaskScheduler {
    pub fn new() -> Self {
        Self(Mutex::new(vec![]))
    }
    pub fn add(&self, task: impl MarkingTask + 'static) {
        self.0.lock().push(Box::new(task));
    }
}
