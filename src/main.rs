use slgc::{heap::Heap, internal::gc_info::*, Config, GCPlatform};
fn main() {
    GCPlatform::initialize();
    let mut config = Config::default();
    config.dump_size_classes = true;
    let (mut heap, mut local) = Heap::new(config);
    for _ in 0..5 {
        let mem = unsafe { local.allocate_raw_or_fail(u32::index(), 16) };
        println!("{:?}", mem);
    }

    for _ in 0..5 {
        let mem = unsafe { local.allocate_raw_or_fail(u32::index(), 75) };
        println!("{:?}", mem);
    }
}
