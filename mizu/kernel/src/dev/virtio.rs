use alloc::sync::Arc;
use core::num::NonZeroU32;

use devices::{intr::Completion, net::Net};
use fdt::{node::FdtNode, Fdt};
use rv39_paging::{PAddr, ID_OFFSET};
use spin::RwLock;
use virtio::{block::VirtioBlock, net::VirtioNet};
use virtio_drivers::transport::{mmio::MmioTransport, DeviceType, Transport};

use super::{block::BLOCKS, interrupts, net::NETS};
use crate::{dev::intr::intr_man, someb, tryb};

pub fn init_mmio(node: &FdtNode, _: &Fdt) -> bool {
    let intr_pin = someb!(interrupts(node).next().and_then(NonZeroU32::new));
    let intr_manager = someb!(intr_man());

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
            let device = Arc::new(device);

            let d2 = device.clone();
            if !intr_manager.insert(intr_pin, move |c| d2.ack_interrupt(c)) {
                return false;
            }

            ksync::critical(|| BLOCKS.lock().push(device));

            true
        }
        DeviceType::Network => {
            let device = tryb!(VirtioNet::<16>::new(mmio).inspect_err(|err| {
                log::debug!("Failed to initialize VirtIO block device: {err}");
            }));
            let device = Arc::new(RwLock::new(device));
            ksync::critical(|| device.write().startup());
            let d2 = device.clone();

            let ack = move |completion: &Completion| {
                ksync::critical(|| loop {
                    if let Some(device) = d2.try_read() {
                        completion();
                        device.ack_interrupt();
                        break true;
                    }
                    core::hint::spin_loop()
                })
            };
            if !intr_manager.insert(intr_pin, ack) {
                return false;
            }

            ksync::critical(|| NETS.lock().push(device));
            true
        }
        ty => {
            log::debug!("Unsupported VirtIO MMIO device type {ty:?}");
            false
        }
    }
}
