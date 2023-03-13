use core::sync::atomic::AtomicUsize;

pub struct SchedInfo {
    pub(crate) last_cpu: AtomicUsize,
}

impl SchedInfo {
    pub fn new() -> Self {
        SchedInfo {
            last_cpu: AtomicUsize::new(0),
        }
    }
}

impl Default for SchedInfo {
    fn default() -> Self {
        Self::new()
    }
}
