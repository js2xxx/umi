use alloc::vec::Vec;

use arsc_rs::Arsc;
use devices::dev::Block;
use spin::Mutex;

pub(in crate::dev) static BLOCKS: Mutex<Vec<Arsc<dyn Block>>> = Mutex::new(Vec::new());
