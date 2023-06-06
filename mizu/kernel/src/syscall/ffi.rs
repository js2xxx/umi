use core::time::Duration;

use ktime::{Instant, InstantExt};

#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct Tv {
    pub sec: u64,
    pub usec: u64,
}

impl From<Instant> for Tv {
    fn from(value: Instant) -> Self {
        let (secs, usecs) = value.to_su();
        Tv {
            sec: secs,
            usec: usecs,
        }
    }
}

impl From<Duration> for Tv {
    fn from(value: Duration) -> Self {
        Tv {
            sec: value.as_secs(),
            usec: value.subsec_micros().into(),
        }
    }
}

impl From<Tv> for Instant {
    fn from(value: Tv) -> Self {
        Instant::from_su(value.sec, value.usec)
    }
}

impl From<Tv> for Duration {
    fn from(value: Tv) -> Self {
        Duration::from_secs(value.sec) + Duration::from_micros(value.usec)
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct Ts {
    pub sec: u64,
    pub nsec: u64,
}

impl From<Instant> for Ts {
    fn from(value: Instant) -> Self {
        let (secs, usecs) = value.to_su();
        Ts {
            sec: secs,
            nsec: usecs * 1000,
        }
    }
}

impl From<Duration> for Ts {
    fn from(value: Duration) -> Self {
        Ts {
            sec: value.as_secs(),
            nsec: value.subsec_nanos().into(),
        }
    }
}

impl From<Ts> for Instant {
    fn from(value: Ts) -> Self {
        Instant::from_su(value.sec, value.nsec / 1000)
    }
}

impl From<Ts> for Duration {
    fn from(value: Ts) -> Self {
        Duration::from_secs(value.sec) + Duration::from_nanos(value.nsec)
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct Itv {
    pub interval: Tv,
    pub next_diff: Tv,
}
