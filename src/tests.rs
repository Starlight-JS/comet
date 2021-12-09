use crate::{
    api::{Collectable, Finalize, Gc, Trace},
    base::GcBase,
    minimark::MiniMarkGC,
};

struct Foo {
    bar: Option<Gc<Bar>>,
}

unsafe impl Trace for Foo {
    fn trace(&mut self, _vis: &mut dyn crate::api::Visitor) {
        self.bar.trace(_vis);
    }
}

unsafe impl Finalize for Foo {}
impl Collectable for Foo {}

pub struct Bar {
    x: u32,
}

unsafe impl Trace for Bar {}
unsafe impl Finalize for Bar {}
impl Collectable for Bar {}

pub struct LargeFoo {
    bar: Option<Gc<Bar>>,
}

unsafe impl Trace for LargeFoo {
    fn trace(&mut self, _vis: &mut dyn crate::api::Visitor) {
        self.bar.trace(_vis);
    }
}

unsafe impl Finalize for LargeFoo {}
impl Collectable for LargeFoo {
    fn allocation_size(&self) -> usize {
        128 * 1024
    }
}

pub struct LargeBar {
    x: u32,
}

unsafe impl Trace for LargeBar {}

unsafe impl Finalize for LargeBar {}
impl Collectable for LargeBar {
    fn allocation_size(&self) -> usize {
        128 * 1024
    }
}

pub struct LargeFoo2 {
    bar: Option<Gc<LargeBar>>,
}

unsafe impl Trace for LargeFoo2 {
    fn trace(&mut self, _vis: &mut dyn crate::api::Visitor) {
        self.bar.trace(_vis);
    }
}

unsafe impl Finalize for LargeFoo2 {}
impl Collectable for LargeFoo2 {
    fn allocation_size(&self) -> usize {
        128 * 1024
    }
}

#[test]
pub fn test_write_barrier() {
    let mut minimark = MiniMarkGC::new(Some(1 * 1024), None, None, false);
    let stack = minimark.shadow_stack();

    letroot!(foo = stack, minimark.allocate(Foo { bar: None }));
    assert!(minimark.is_young(*foo));
    minimark.minor_collection(&mut []);

    assert!(!minimark.is_young(*foo));

    let bar = minimark.allocate(Bar { x: 420 });
    assert!(minimark.is_young(bar));
    foo.bar = Some(bar);
    minimark.write_barrier(*foo);

    minimark.minor_collection(&mut []);
    assert_eq!(foo.bar.as_ref().unwrap().x, 420);
    assert!(!minimark.is_young(*foo.bar.as_ref().unwrap()));
}

#[test]
pub fn test_write_barrier_large() {
    let mut minimark = MiniMarkGC::new(Some(1 * 1024), None, None, false);
    let stack = minimark.shadow_stack();

    letroot!(foo = stack, minimark.allocate(LargeFoo { bar: None }));
    assert!(minimark.is_young(*foo));
    minimark.minor_collection(&mut []);

    assert!(!minimark.is_young(*foo));

    let bar = minimark.allocate(Bar { x: 420 });
    assert!(minimark.is_young(bar));
    foo.bar = Some(bar);
    minimark.write_barrier(*foo);

    minimark.minor_collection(&mut []);
    assert_eq!(foo.bar.as_ref().unwrap().x, 420);
    assert!(!minimark.is_young(*foo.bar.as_ref().unwrap()));
}

#[test]
pub fn test_write_barrier_large_2() {
    let mut minimark = MiniMarkGC::new(Some(1 * 1024), None, None, false);
    let stack = minimark.shadow_stack();

    letroot!(foo = stack, minimark.allocate(LargeFoo2 { bar: None }));
    assert!(minimark.is_young(*foo));
    minimark.minor_collection(&mut []);

    assert!(!minimark.is_young(*foo));

    let bar = minimark.allocate(LargeBar { x: 420 });
    assert!(minimark.is_young(bar));
    foo.bar = Some(bar);
    minimark.write_barrier(*foo);

    minimark.minor_collection(&mut []);
    assert_eq!(foo.bar.as_ref().unwrap().x, 420);
    assert!(!minimark.is_young(*foo.bar.as_ref().unwrap()));
}
