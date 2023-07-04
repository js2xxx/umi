use core::sync::atomic::{AtomicI32, Ordering::SeqCst};

use hashbrown::HashMap;
use kmem::Phys;
use ksc::Error::{self, EEXIST, ENOENT};
use rand_riscv::RandomState;
use rv39_paging::LAddr;
use spin::{mutex::Mutex, MutexGuard};

pub struct Shm {
    id_alloc: AtomicI32,
    map: Mutex<HashMap<i32, (Phys, usize), RandomState>>,
    mapping: Mutex<HashMap<LAddr, LAddr, RandomState>>,
}

impl Default for Shm {
    fn default() -> Self {
        Shm {
            id_alloc: AtomicI32::new(2),
            map: Default::default(),
            mapping: Default::default(),
        }
    }
}

impl Shm {
    pub fn get(&self, key: i32) -> Option<(Phys, usize)> {
        ksync::critical(|| self.map.lock().get(&key).cloned())
    }

    pub fn insert(&self, key: i32, len: usize, flags: i32) -> Result<i32, Error> {
        const IPC_CREAT: i32 = 0o1000;
        const IPC_EXCL: i32 = 0o2000;

        if key == 0 {
            let key = self.id_alloc.fetch_add(1, SeqCst);
            ksync::critical(|| self.map.lock().insert(key, (Phys::new(false), len)));
            Ok(key)
        } else if flags & IPC_CREAT == 0 {
            ksync::critical(|| self.map.lock().contains_key(&key))
                .then_some(key)
                .ok_or(ENOENT)
        } else if flags & IPC_EXCL == 0 {
            ksync::critical(|| {
                self.map
                    .lock()
                    .entry(key)
                    .or_insert_with(|| (Phys::new(false), len));
                Ok(key)
            })
        } else {
            ksync::critical(|| {
                self.map
                    .lock()
                    .try_insert(key, (Phys::new(false), len))
                    .is_ok()
            })
            .then_some(key)
            .ok_or(EEXIST)
        }
    }

    pub fn mapping(&self) -> MutexGuard<HashMap<LAddr, LAddr, RandomState>> {
        self.mapping.lock()
    }
}
