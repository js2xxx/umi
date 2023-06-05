use alloc::sync::{Arc, Weak};
use core::{
    sync::atomic::{
        AtomicU64,
        Ordering::{Relaxed, SeqCst},
    },
    time::Duration,
};

use ktime::{Instant, InstantExt};
use sygnal::{Sig, SigCode, SigFields, SigInfo};

const USER: usize = 0;
const SYSTEM: usize = 1;

#[derive(Debug, Default)]
pub struct Times {
    tgroup_submitter: Weak<Times>,

    me: [AtomicU64; 2],
    process: [AtomicU64; 2],
    children: [AtomicU64; 2],
}

impl Times {
    pub fn new_thread(tgroup: &Arc<Times>) -> Arc<Self> {
        Arc::new(Times {
            tgroup_submitter: Arc::downgrade(tgroup),
            ..Default::default()
        })
    }

    fn update<const INDEX: usize>(&self, delta: u64) {
        self.me[INDEX].fetch_add(delta, SeqCst);
        match self.tgroup_submitter.upgrade() {
            Some(tgroup) => tgroup.process[INDEX].fetch_add(delta, SeqCst),
            None => self.process[INDEX].fetch_add(delta, SeqCst),
        };
    }

    pub fn update_user(&self, delta: u64) {
        self.update::<USER>(delta)
    }

    pub fn update_system(&self, delta: u64) {
        self.update::<SYSTEM>(delta)
    }

    pub fn append_child(&self, child: &Times) {
        self.update_user(child.me[USER].load(SeqCst));
        self.update_system(child.me[SYSTEM].load(SeqCst));
    }

    pub fn get(&self, process: bool) -> [u64; 4] {
        let mut storage = None;
        let times = match self.tgroup_submitter.upgrade() {
            Some(tg) if process => &*storage.insert(tg),
            _ => self,
        };
        [
            times.me[USER].load(Relaxed),
            times.me[SYSTEM].load(Relaxed),
            times.children[USER].load(Relaxed),
            times.children[SYSTEM].load(Relaxed),
        ]
    }

    fn get_process_raw(&self) -> [u64; 2] {
        let mut storage = None;
        let times = match self.tgroup_submitter.upgrade() {
            Some(tg) => &*storage.insert(tg),
            _ => self,
        };
        times.process.each_ref().map(|s| s.load(Relaxed))
    }

    pub fn get_process(&self) -> [Duration; 2] {
        self.get_process_raw().map(config::to_duration)
    }

    pub fn get_thread(&self) -> [Duration; 2] {
        self.me
            .each_ref()
            .map(|s| config::to_duration(s.load(Relaxed)))
    }

    pub fn get_children(&self) -> [Duration; 2] {
        let mut storage = None;
        let times = match self.tgroup_submitter.upgrade() {
            Some(tg) => &*storage.insert(tg),
            _ => self,
        };
        times
            .children
            .each_ref()
            .map(|s| config::to_duration(s.load(Relaxed)))
    }
}

#[derive(Debug)]
pub struct Counter {
    interval: Duration,
    next_tick: Instant,
    now: fn(&Times) -> Instant,
    sig: Sig,
}

impl Counter {
    pub fn new_real() -> Self {
        Counter {
            interval: Duration::ZERO,
            next_tick: Instant::now(),
            now: |_| Instant::now(),
            sig: Sig::SIGALRM,
        }
    }

    pub fn new_virtual() -> Self {
        Counter {
            interval: Duration::ZERO,
            next_tick: Instant::from_su(0, 0),
            now: |times| {
                let [user, _] = times.get_process_raw();
                unsafe { Instant::from_raw(user) }
            },
            sig: Sig::SIGVTALRM,
        }
    }

    pub fn new_profile() -> Self {
        Counter {
            interval: Duration::ZERO,
            next_tick: Instant::from_su(0, 0),
            now: |times| {
                let [user, system] = times.get_process_raw();
                unsafe { Instant::from_raw(user + system) }
            },
            sig: Sig::SIGPROF,
        }
    }

    pub fn update(&mut self, times: &Times) -> Option<SigInfo> {
        let now = (self.now)(times);
        if self.interval.is_zero() {
            return None;
        }
        if self.next_tick >= now {
            self.next_tick = now + self.interval;
            return Some(SigInfo {
                sig: self.sig,
                code: SigCode::TIMER as _,
                fields: SigFields::None,
            });
        }
        None
    }

    pub fn set(
        &mut self,
        times: &Times,
        set: Option<(Duration, Duration)>,
    ) -> (Duration, Duration) {
        let now = (self.now)(times);
        let old = (self.interval, self.next_tick - now);
        if let Some((interval, next_diff)) = set {
            self.interval = interval;
            self.next_tick = now + next_diff;
        }
        old
    }
}

pub fn counters() -> [Counter; 3] {
    [
        Counter::new_real(),
        Counter::new_virtual(),
        Counter::new_profile(),
    ]
}
