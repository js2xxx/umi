use alloc::sync::Arc;
use core::num::NonZeroU32;

use fdt::{node::FdtNode, Fdt};
use rv39_paging::{PAddr, ID_OFFSET};
use sdmmc::Sdmmc;

use super::intr::intr_man;
use crate::{
    dev::{block::BLOCKS, interrupts},
    someb, tryb,
};

pub fn init(node: &FdtNode, _: &Fdt) -> bool {
    let intr_pin = someb!(interrupts(node).next().and_then(NonZeroU32::new));
    let intr_manager = someb!(intr_man());

    let reg = someb!(node.reg().and_then(|mut reg| reg.next()));
    let addr = PAddr::new(reg.starting_address as _).to_laddr(ID_OFFSET);

    let bus_width = someb!(node.property("bus-width").and_then(|p| p.as_usize()));
    let min_freq = someb!(node.property("min-frequency").and_then(|p| p.as_usize())) as u64;
    let max_freq = someb!(node.property("max-frequency").and_then(|p| p.as_usize())) as u64;

    let sdmmc = tryb!(unsafe { Sdmmc::new(addr.as_non_null_unchecked().cast()) }
        .inspect_err(|err| log::debug!("failed to initialize SDMMC structure: {err:?}")));
    tryb!(sdmmc
        .init_bus(bus_width, min_freq..max_freq)
        .inspect_err(|err| log::debug!("failed to initialize SDMMC bus: {err:?}")));

    let device = Arc::new(sdmmc);
    let d2 = device.clone();
    assert!(intr_manager.insert(intr_pin, move |c| d2.ack_interrupt(c)));

    ksync::critical(|| BLOCKS.lock().push(device));

    true
}
