use crate::{NR_ENTRIES, NR_ENTRIES_SHIFT, PAGE_SHIFT};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Level(u8);

impl Level {
    pub const fn new(level: u8) -> Self {
        assert!(level < 3, "RISC-V sv39 paging only supports 3 levels");
        Self(level)
    }

    pub const fn pt() -> Self {
        Self::new(0)
    }

    pub const fn max() -> Self {
        Self::new(2)
    }

    #[inline]
    pub const fn page_shift(&self) -> usize {
        PAGE_SHIFT + self.0 as usize * NR_ENTRIES_SHIFT
    }

    #[inline]
    pub const fn page_size(&self) -> usize {
        1usize << self.page_shift()
    }

    #[inline]
    pub const fn page_mask(&self) -> usize {
        self.page_size() - 1
    }

    #[inline]
    pub const fn paddr_mask(&self) -> usize {
        ((1 << 56) - 1) & !self.page_mask()
    }

    #[inline]
    pub const fn laddr_mask(&self) -> usize {
        Level(3).page_mask() & !self.page_mask()
    }

    #[inline]
    pub const fn addr_idx(&self, laddr: usize, end: bool) -> usize {
        let ret = ((laddr & self.laddr_mask()) >> self.page_shift()) & (NR_ENTRIES - 1);
        if end && ret == 0 {
            NR_ENTRIES
        } else {
            ret
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Level, PAGE_SHIFT, PAGE_SIZE};

    #[test]
    fn test_page_size() {
        assert_eq!(PAGE_SHIFT, Level::pt().page_shift());
        assert_eq!(PAGE_SIZE, Level::pt().page_size());
    }
}
