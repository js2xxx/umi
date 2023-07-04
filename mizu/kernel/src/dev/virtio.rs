use alloc::sync::Arc;
use core::num::NonZeroU32;

use devices::block::Block;
use fdt::node::FdtNode;
use rv39_paging::{PAddr, ID_OFFSET};
use virtio::block::VirtioBlock;
use virtio_drivers::transport::{mmio::MmioTransport, DeviceType, Transport};

use super::block::BLOCKS;
use crate::{dev::intr::intr_man, executor, someb, tryb};

pub fn init_mmio(node: &FdtNode) -> bool {
    let intr_pin = someb!(node
        .interrupts()
        .and_then(|mut intr| intr.next())
        .and_then(|pin| pin.try_into().ok())
        .and_then(NonZeroU32::new));
    let intr_manager = &someb!(intr_man());

    let reg = someb!(node.reg().and_then(|mut reg| reg.next()));
    let addr = PAddr::new(reg.starting_address as _).to_laddr(ID_OFFSET);

    let header = someb!(addr.as_non_null());
    let mmio = tryb!(unsafe {
        MmioTransport::new(header.cast()).inspect_err(|err| {
            log::trace!("Invalid VirtIO MMIO header: {err}");
        })
    });

    match mmio.device_type() {
        DeviceType::Block => {
            let device = tryb!(VirtioBlock::new(mmio).inspect_err(|err| {
                log::debug!("Failed to initialize VirtIO block device: {err}");
            }));
            let intr = someb!(intr_manager.insert(intr_pin));

            let device = Arc::new(device);
            executor()
                .spawn(device.clone().intr_dispatch(intr))
                .detach();
            ksync::critical(|| BLOCKS.lock().push(device));

            true
        }
        ty => {
            log::debug!("Unsupported VirtIO MMIO device type {ty:?}");
            false
        }
    }
}
