pub const fn build_range<const N: usize>(start: u16, step: u16) -> [u16; N] {
    let mut arr = [0u16; N];
    let mut i = 0;
    while i < N {
        arr[i] = start + (i as u16) * step;
        i += 1;
    }
    arr
}

pub const fn fold_sum(mut sum: u32) -> u16 {
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    sum as u16
}

pub fn sum_byte_arr(buf: &[u8]) -> u32 {
    let mut sum = 0;
    for chunk in buf.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], 0])
        };
        sum += word as u32;
    }
    sum
}
