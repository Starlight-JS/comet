pub struct HeapStatistics {
    pub memory_allocated_for_immix_blocks: usize,
    pub immix_space_size: usize,
    pub memory_allocated_for_large_space: usize,
    pub immix_blocks: usize,
    pub large_allocations: usize,
    pub total_gc_cycles_count: usize,
    pub total_memory_allocated: usize,
    pub total_objects_found_on_stack: usize,
    pub total_objects_allocated: usize,
    pub heap_threshold: usize,
}

struct FormattedSize {
    pub size: usize,
}

impl std::fmt::Display for FormattedSize {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let ksize = (self.size as f64) / 1024f64;

        if ksize < 1f64 {
            return write!(f, "{}B", self.size);
        }

        let msize = ksize / 1024f64;

        if msize < 1f64 {
            return write!(f, "{:.1}K", ksize);
        }

        let gsize = msize / 1024f64;

        if gsize < 1f64 {
            write!(f, "{:.1}M", msize)
        } else {
            write!(f, "{:.1}G", gsize)
        }
    }
}

fn formatted_size(size: usize) -> FormattedSize {
    FormattedSize { size }
}

impl std::fmt::Display for HeapStatistics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Heap statistics:")?;
        writeln!(
            f,
            "  Memory allocated for immix space: {} of {}",
            formatted_size(self.memory_allocated_for_immix_blocks),
            formatted_size(self.immix_space_size)
        )?;
        writeln!(
            f,
            "  Memory allocated for large space: {}",
            formatted_size(self.memory_allocated_for_large_space)
        )?;
        writeln!(f, "  Immix blocks allocated: {}", self.immix_blocks)?;
        writeln!(f, "  Large allocations: {}", self.large_allocations)?;
        writeln!(
            f,
            "  Current memory usage: {}",
            formatted_size(
                self.memory_allocated_for_immix_blocks + self.memory_allocated_for_large_space
            )
        )?;
        writeln!(f, "  Total GC cycles count: {}", self.total_gc_cycles_count)?;
        writeln!(
            f,
            "  Total memory allocated: {}",
            formatted_size(self.total_memory_allocated)
        )?;
        writeln!(
            f,
            "  Total objects allocated: {}",
            self.total_objects_allocated
        )?;
        writeln!(
            f,
            "  Total objects found conservatively: {} ({:.2}%)",
            self.total_objects_found_on_stack,
            (self.total_objects_found_on_stack as f64 / self.total_objects_allocated as f64)
                * 100.0
        )?;
        writeln!(
            f,
            "  Heap threshold: {}",
            formatted_size(self.heap_threshold)
        )?;
        Ok(())
    }
}
