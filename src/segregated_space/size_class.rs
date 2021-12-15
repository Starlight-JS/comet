use super::page::SegregatedSpacePage;

pub const SIZE_STEP: usize = 16;
pub const LARGE_CUTOFF: usize = (SegregatedSpacePage::PAYLOAD / 2) & !(SIZE_STEP - 1);
pub const NUM_SIZE_CLASSES: usize = LARGE_CUTOFF / SIZE_STEP + 1;
pub const PRECISE_CUTOFF: usize = 80;
pub const fn size_class_to_index(size: usize) -> usize {
    (size + SIZE_STEP - 1) / SIZE_STEP
}

pub const fn index_to_size_class(index: usize) -> usize {
    index * SIZE_STEP
}

const fn round_up_to_multiple_of(divisor: usize, x: usize) -> usize {
    (x + (divisor - 1)) & !(divisor - 1)
}

pub fn size_classes(size_class_progression: f64, dump: bool) -> Vec<usize> {
    let mut result = Vec::new();
    let add = |result: &mut Vec<usize>, mut size_class| {
        size_class = round_up_to_multiple_of(SIZE_STEP, size_class) as usize;
        if dump {
            println!("Adding size class: {}", size_class);
        }
        result.push(size_class);
    };

    let mut size = SIZE_STEP;
    while size < PRECISE_CUTOFF {
        add(&mut result, size);
        size += SIZE_STEP;
    }

    if dump {
        println!(
            "segregated space page payload: {}",
            SegregatedSpacePage::PAYLOAD
        );
    }
    for i in 0.. {
        let approximate_size = PRECISE_CUTOFF as f64 * size_class_progression.powi(i);
        if dump {
            println!("next size class as a f64: {}", approximate_size);
        }

        let approximate_size_in_bytes = approximate_size as usize;
        if dump {
            println!("next size class as bytes: {}", approximate_size_in_bytes);
        }

        if approximate_size_in_bytes > LARGE_CUTOFF {
            break;
        }

        let size_class =
            round_up_to_multiple_of(SIZE_STEP, approximate_size_in_bytes as _) as usize;

        if dump {
            println!("size class: {}", size_class);
        }

        if Some(&size_class) == result.last() {
            continue;
        }

        add(&mut result, size_class);
    }

    add(&mut result, 256);
    result.sort_unstable();
    result.dedup();
    if dump {
        println!("size class dump: {:?}", result);
    }
    result
}
