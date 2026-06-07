pub struct Bitmap<'a> {
    data: &'a mut [u8],
    bits: usize,
}

impl<'a> Bitmap<'a> {
    pub fn get(&mut self, bit: usize) -> bool {
        debug_assert!(bit < self.bits);
        (self.data[bit / 8] >> (bit % 8)) & 1 > 0
    }

    pub fn set(&mut self, bit: usize) {
        debug_assert!(bit < self.bits);
        self.data[bit / 8] |= 1 << (bit % 8);
    }

    pub fn clear(&mut self, bit: usize) {
        debug_assert!(bit < self.bits);
        self.data[bit / 8] &= !(1 << (bit % 8));
    }

    pub fn first_empty(&self) -> Option<usize> {
        for (byte_idx, &byte) in self.data.iter().enumerate() {
            if byte != 0xFF {
                let bit_idx = byte.trailing_ones() as usize;
                let idx = byte_idx * 8 + bit_idx;
                if idx < self.bits {
                    return Some(idx);
                }
            }
        }

        None
    }
}
