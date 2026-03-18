pub const fn build_range<const N: usize>(start: u16, step: u16) -> [u16; N] {
    let mut arr = [0u16; N];
    let mut i = 0;
    while i < N {
        arr[i] = start + (i as u16) * step;
        i += 1;
    }
    arr
}
