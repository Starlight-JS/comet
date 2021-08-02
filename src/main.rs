use std::time::Duration;

use slgc::{heap::Heap, internal::gc_info::*, Config, GCPlatform};
fn main() {
    GCPlatform::initialize();
    let mut config = Config::default();
    config.dump_size_classes = true;
    config.verbose = true;
    config.block_threshold = 1;
    let (_heap, mut local) = Heap::new(config);
    unsafe {
        let mem = local.allocate_raw_or_fail(u32::index(), 48);
        let mem2 = local.allocate_raw_or_fail(u32::index(), 16 * 1024);
        local.try_perform_collection();
        let mem3 = local.allocate_raw_or_fail(u16::index(), 129);
        local.try_perform_collection();

        println!("{:p} {:p} {:p}", &mem, &mem2, &mem3);
    }
}
