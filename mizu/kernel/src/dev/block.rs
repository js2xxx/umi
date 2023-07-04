use alloc::{sync::Arc, vec::Vec};

use devices::block::Block;
use spin::Mutex;

pub static BLOCKS: Mutex<Vec<Arc<dyn Block>>> = Mutex::new(Vec::new());

pub fn block(index: usize) -> Option<Arc<dyn Block>> {
    ksync::critical(|| BLOCKS.lock().get(index).cloned())
}

pub fn blocks() -> Vec<Arc<dyn Block>> {
    ksync::critical(|| BLOCKS.lock().clone())
}
