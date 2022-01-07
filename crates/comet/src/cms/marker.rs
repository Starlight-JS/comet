use super::marking_worklist::MarkingWorklists;

pub struct Marker {
    marking_worklists: MarkingWorklists,
    is_marking: bool,
}

impl Marker {
    pub fn marking_worklists(&self) -> &MarkingWorklists {
        &self.marking_worklists
    }
}
