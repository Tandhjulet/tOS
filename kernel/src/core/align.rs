pub fn align_down(value: usize, align: usize) -> usize {
    assert!(align.is_power_of_two(), "`align` must be a power of two");
    value & !(align - 1)
}

pub fn align_up(value: usize, align: usize) -> usize {
    assert!(align.is_power_of_two(), "`align` must be a power of two");
    (value + align - 1) & !(align - 1)
}
