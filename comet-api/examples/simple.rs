use comet::GCPlatform;
use comet_api::{*,cell::Heap};


struct Node<T: TraceTrait + 'static + std::fmt::Debug> {
    next: Option<GcPointer<Node<T>>>,
    value: T
}

impl<T: Trace + 'static + std::fmt::Debug> GcCell for Node<T> {}

impl<T: Trace + std::fmt::Debug> Trace for Node<T> {
    fn trace(&self, vis: &mut Visitor) {
        println!("Trace {:p} {:?}",self,self);
        self.value.trace(vis);
        self.next.trace(vis);
    }
}
impl<T: Trace + std::fmt::Debug> Finalize<Self> for Node<T> {}

impl<T: std::fmt::Debug + Trace> std::fmt::Debug for Node<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f,"Node {{next: {:?}, value: {:?} }}",self.next,self.value)
    }
}

impl<T: std::fmt::Debug + Trace> Drop for Node<T> {
    fn drop(&mut self) {
        println!("drop {:p} {:?}",self,self);
    }
}

fn main() {
    GCPlatform::initialize();

    let mut heap = Heap::new(None);

    let mut head = heap.allocate(Node {next: None,value: 0i32 });
    head.next = Some(heap.allocate(Node {next: None, value: 1}));
    heap.gc();

    println!("{:p}",&head);
    head.next = None;
    heap.gc();
    println!("{:p}", &head);
}