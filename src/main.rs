use comet::{global_allocator::LARGE_CUTOFF, heap::Heap, internal::gc_info::*, Config, GCPlatform};
fn main() {
    GCPlatform::initialize();

    let mut config = Config::default();
    config.dump_size_classes = true;
    config.verbose = true;
    config.generational = true;
    let (_heap, mut local) = Heap::new(config);
    _heap.add_core_constraints(&local);
    unsafe {
        let mem = local.allocate_raw_or_fail(u32::index(), 48);

        let mem3 = local.allocate_weak_ref(mem);
        local.try_perform_collection();
        println!("{:?}", mem3.upgrade());
    }
}
