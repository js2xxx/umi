use alloc::sync::{Arc, Weak};
use core::{
    sync::atomic::{
        AtomicU64,
        Ordering::{Relaxed, SeqCst},
    },
    time::Duration,
};

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

    pub fn get_process(&self) -> [Duration; 2] {
        let mut storage = None;
        let times = match self.tgroup_submitter.upgrade() {
            Some(tg) => &*storage.insert(tg),
            _ => self,
        };
        times
            .process
            .each_ref()
            .map(|s| config::to_duration(s.load(Relaxed)))
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
