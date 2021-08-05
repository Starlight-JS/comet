use slgc::{global_allocator::LARGE_CUTOFF, heap::Heap, internal::gc_info::*, Config, GCPlatform};
fn main() {
    GCPlatform::initialize();
    println!("{}", LARGE_CUTOFF);
    let mut config = Config::default();
    config.dump_size_classes = true;
    config.verbose = true;
    config.generational = true;
    let (_heap, mut local) = Heap::new(config);
    _heap.add_core_constraints(&local);
    unsafe {
        let mem = local.allocate_raw_or_fail(u32::index(), 48);

        let mem3 = local.allocate_raw_or_fail(u16::index(), 129);
        local.try_perform_collection();
        local.try_perform_collection();
    }
}
