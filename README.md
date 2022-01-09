# Comet

Comet is a collection of various [trace-based garbage collectors](https://en.wikipedia.org/wiki/Tracing_garbage_collection) that can be used to implement programming language runtime in Rust. Originally comet was part of [Starlight](https://github.com/starligt-js/starlight), JS engine in Rust, but then it was extracted to a separate crate. Since then a lot of features were added to it: 
- Support for multi-threading
- Support for multiple GC heaps in one process
- Generational garbage collection
- Moving garbage collection
- More than one GC policies

# Why Comet?

Comet is fast & easy to use GC library and you can simply plug it in into your runtime with as less hassle as possible. Comet implements a few GC policies where each has their own tradeoffs but generally all of the policies are quite performant. 


# Precise collection

All the GC policies are 'precise' in that they know the layout of allocations (which is used to determine reachable children) and also the location of all stack roots. This means they do not need to resort to conservative techniques that may cause garbage to be retained unnecessarily. To keep stack roots we use shadow stack and for use of Comet you ***must*** read ROOTING.md


# Finalization support

Comet supports invoking object finalizers but it does not support "complex" finalizers i.e finalizers that might need special ordering of execution or might revive object. If your finalizer revies object it is UB. Also using finalizers slow downs your program and you should use allocate finalizeable objects on GC heap in very rare cases like file handles. If you need replacement for `std` containers you can use `comet_extra::alloc` module that provides properly GC allocated container types. 

## GC Policies

### SemiSpace

Simple semi-space collector that divides heap into two spaces: to space and from sapce.  Garbage collection is performed by copying live objects from one semispace (the from-space) to the other (the to-space), which then becomes the new heap. The entire old heap is then discarded in one piece.

## MarkSweep

Naive Mark&Sweep garbage collector that allocates memory in [rosalloc](https://github.com/playxe/rosalloc) and when certain GC threshold is reached performs garbage collection. Quite slow compared to all the others GCs.

## MiniMark

Generational garbage collector that has two generations: nursery and old space. Initially all objects are allocated into nursery (unless explicitly specified). Once nursery becomes full, all surviving objects
are promoted to old space, and once old space is full old space is collected in mark&sweep fashion. 

## Immix 

The Immix is a mark-region garbage collector. It is based on mark-and-sweep but instead of sweeping on per object basis it sweeps lines which are 256 bytes in size. This allows us to always bump-allocate memory from "holes" (hole is a region with unmarked lines) and gives nice cache locality to allocations performed near each other. If you want to learn more you can read this [paper](https://users.cecs.anu.edu.au/~steveb/pubs/papers/immix-pldi-2008.pdf). 


# Which GC policy to choose? 

The golden middle is Immix, it has relatively good latency, high throughput and good cache locality but it does not compact memory (yet) and might require larger than you need heap sizes (min heap size is 4MB). In case you want non-moving GC then MarkSweep is the best and the only choice at the moment, it allows you to create GC heap that is small in size (heap might be as small as 64KB) and it is guaranteed to not move objects in memory which might be useful for FFI (although moving collectors can be used for FFI too, but with more complex FFI Handles implementation). And finally we're reached MiniMark, this GC is generational and it is well suited for quite every application but it comes at the cost of maintaining write barrier that should be inserted after each write to GC object. This GC also has relatively large heap sizes although you can set nursery size to just 128KB and old space size to 1MB but then it becomes useless in such small heap sizes.


By the way, what's about SemiSpace, should I use it? Answer is: probably no. It does provide good cache benefits but requires 2X heap size for GC cycle and it is usually not much faster than Immix/MiniMark in real world workloads. The main purpose it exists in Comet is just to demonstrate simple GC implementation.