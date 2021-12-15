use std::mem::size_of;

pub struct SegregatedSpacePage {}

impl SegregatedSpacePage {
    pub const SIZE: usize = 128 * 1024;
    pub const PAYLOAD: usize = Self::SIZE - size_of::<Self>();
}
