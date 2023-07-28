use alloc::{sync::Arc, vec::Vec};

use devices::net::Net;
use spin::{Mutex, RwLock};

pub(super) static NETS: Mutex<Vec<Arc<RwLock<dyn Net>>>> = Mutex::new(Vec::new());

#[allow(dead_code)]
pub fn net(index: usize) -> Option<Arc<RwLock<dyn Net>>> {
    ksync::critical(|| NETS.lock().get(index).cloned())
}

pub fn nets() -> Vec<Arc<RwLock<dyn Net>>> {
    ksync::critical(|| NETS.lock().clone())
}
