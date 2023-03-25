use crate::{NR_ENTRIES, NR_ENTRIES_SHIFT, PAGE_SHIFT};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Level(u8); // the u8 indicates what level it is.

// something about rv pgtbl levels, 2->0 from root to leaf
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

    /// Return 0...011...1 (12+9n *1*s) if `self.0 = n`
    #[inline]
    pub const fn page_mask(&self) -> usize {
        self.page_size() - 1
    }

    /// Return 1...100...0 (12+9n *0*s) if `self.0 = n`
    #[inline]
    pub const fn paddr_mask(&self) -> usize {
        ((1 << 56) - 1) & !self.page_mask()
    }

    #[inline]
    pub const fn laddr_mask(&self) -> usize {
        Level(3).page_mask() & !self.page_mask()
    }

    /// Return PPN based on level with given la and end
    ///
    /// Example: if level = 0, return the lowest 9-bit PPN of la
    #[inline]
    pub const fn addr_idx(&self, laddr: usize, end: bool) -> usize {
        let ret = ((laddr & self.laddr_mask()) >> self.page_shift()) & (NR_ENTRIES - 1);
        if end && ret == 0 {
            NR_ENTRIES
        } else {
            ret
        }
    }

    #[inline]
    pub const fn decrease(&self) -> Option<Level> {
        self.0.checked_sub(1).map(Level)
    }
}

#[const_trait]
pub trait AddrExt {
    fn round_down(self, level: Level) -> Self;

    fn round_up(self, level: Level) -> Self;
}

impl const AddrExt for usize {
    #[inline]
    fn round_down(self, level: Level) -> Self {
        self & !level.page_mask()
    }

    #[inline]
    fn round_up(self, level: Level) -> Self {
        (self + level.page_mask()) & !level.page_mask()
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
