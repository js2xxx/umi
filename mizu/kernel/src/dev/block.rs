use alloc::vec::Vec;

use arsc_rs::Arsc;
use devices::dev::Block;
use spin::Mutex;

pub static BLOCKS: Mutex<Vec<Arsc<dyn Block>>> = Mutex::new(Vec::new());

pub fn block(index: usize) -> Option<Arsc<dyn Block>> {
    BLOCKS.lock().get(index).cloned()
}

pub fn blocks() -> Vec<Arsc<dyn Block>> {
    BLOCKS.lock().clone()
}
