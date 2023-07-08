use ktime::InstantExt;

pub fn instant_to_smoltcp(src: ktime::Instant) -> smoltcp::time::Instant {
    let (s, u) = src.to_su();
    smoltcp::time::Instant::from_micros((u + s * 1_000_000) as i64)
}

pub fn instant_from_smoltcp(src: smoltcp::time::Instant) -> ktime::Instant {
    let u = src.micros() as u64;
    ktime::Instant::from_su(u / 1_000_000, u % 1_000_000)
}

pub fn duration_to_smoltcp(src: core::time::Duration) -> smoltcp::time::Duration {
    smoltcp::time::Duration::from_micros(src.as_micros() as u64)
}

pub fn duration_from_smoltcp(src: smoltcp::time::Duration) -> core::time::Duration {
    core::time::Duration::from_micros(src.micros())
}
