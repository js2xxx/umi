use core::{
    fmt,
    ops::{Add, AddAssign, Sub, SubAssign},
    time::Duration,
};

use crate::InstantExt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(u128);

impl Instant {
    pub fn now() -> Self {
        // SAFETY: The raw value is valid.
        unsafe { Self::from_raw(Self::now_raw()) }
    }

    /// Used for atomic storages.
    pub fn now_raw() -> u64 {
        riscv::register::time::read64()
    }

    /// # Safety
    ///
    /// The `raw` must be a valid value that can be transformed into an instant.
    pub unsafe fn from_raw(raw: u64) -> Self {
        let micros = config::TIME_FREQ_M.numer() * raw as u128 / config::TIME_FREQ_M.denom();
        Instant(micros)
    }

    #[must_use]
    pub fn checked_duration_since(&self, earlier: Self) -> Option<Duration> {
        let micros = self.0.checked_sub(earlier.0)?;
        let secs = (micros / 1_000_000) as u64;
        let micros = (micros % 1_000_000) as u32;
        Some(Duration::new(secs, micros))
    }

    #[must_use]
    pub fn duration_since(&self, earlier: Self) -> Duration {
        self.checked_duration_since(earlier).unwrap_or_default()
    }

    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration.as_micros()).map(Instant)
    }

    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        self.0.checked_sub(duration.as_micros()).map(Instant)
    }

    #[inline]
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        Instant::now() - *self
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;

    fn add(self, rhs: Duration) -> Self::Output {
        self.checked_add(rhs)
            .expect("overflow when adding duration to instant")
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, rhs: Duration) {
        *self = *self + rhs;
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;

    fn sub(self, rhs: Duration) -> Self::Output {
        self.checked_sub(rhs)
            .expect("overflow when substracting duration to instant")
    }
}

impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, rhs: Duration) {
        *self = *self - rhs;
    }
}

impl Sub for Instant {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Self::Output {
        self.duration_since(rhs)
    }
}

impl fmt::Debug for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (secs, usecs) = self.to_su();
        write!(f, "{secs}.{usecs:06}")
    }
}

impl super::InstantExt for Instant {
    fn to_su(&self) -> (u64, u64) {
        ((self.0 / 1_000_000) as u64, (self.0 % 1_000_000) as u64)
    }

    fn from_su(secs: u64, micros: u64) -> Self {
        Instant(secs as u128 * 1_000_000 + micros as u128)
    }
}
