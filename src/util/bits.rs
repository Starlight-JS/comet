#[inline]
pub fn which_power_of_two(value: usize) -> usize {
    value.trailing_zeros() as _
}

pub fn round_up_to_power_of_two32(mut value: u32) -> u32 {
    if value > 0 {
        value -= 1;
    }
    1 << (32 - value.leading_zeros())
}
#[inline]
pub fn round_down_to_power_of_two32(value: u32) -> u32 {
    if value > 0x80000000 {
        return 0x80000000;
    }

    let mut result = round_up_to_power_of_two32(value);
    if result > value {
        result >>= 1;
    }
    result
}
