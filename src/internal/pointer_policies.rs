pub trait WriteBarrierPolicyTrait {
    fn initializing_barrier(_: *const u8, _: *const u8) {}
    fn assigning_barrier(_: *const u8, _: *const u8) {}
}

pub struct DijkstraWriteBarrierPolicy;

impl WriteBarrierPolicyTrait for DijkstraWriteBarrierPolicy {
    fn initializing_barrier(_: *const u8, _: *const u8) {
        // Since in initializing writes the source object is always white, having no
        // barrier doesn't break the tri-color invariant.
    }
    fn assigning_barrier(slot: *const u8, value: *const u8) {
        eprintln!("todo: write barrier {:p}<-{:p}", slot, value);
    }
}

pub struct NoWriteBarrierPolicy;

impl WriteBarrierPolicyTrait for NoWriteBarrierPolicy {}

pub struct StrongMemberTag;
pub struct WeakMemberTag;
pub struct UntracedMemberTag;
