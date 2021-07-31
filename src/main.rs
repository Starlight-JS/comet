use slgc::{internal::gc_info::*, GCPlatform};
fn main() {
    GCPlatform::initialize();
    println!("{}", u32::index());
    println!("{}", u16::index());
    println!("{}", u32::index());
    println!("{}", u16::index());
    println!("{}", u8::index());
}
