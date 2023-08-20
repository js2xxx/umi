mod block;
mod intr;
mod net;
mod sdmmc;
mod serial;
mod virtio;

use alloc::vec::Vec;
use core::mem;

use fdt::{node::FdtNode, Fdt, FdtError};
use ksc::Handlers;
use spin::{Lazy, Once};

pub use self::{
    block::{block, blocks},
    intr::INTR,
    net::{net, nets},
    serial::{init_logger, Stdin, Stdout},
};

static DEV_INIT: Lazy<Handlers<&str, (&FdtNode, &Fdt), bool>> = Lazy::new(|| {
    Handlers::new()
        .map("ns16550a", serial::init_ns16550a)
        .map("snps,dw-apb-uart", serial::init_dw_apb_uart)
        .map("riscv,plic0", intr::init_plic)
        .map("virtio,mmio", virtio::init_mmio)
        .map("cvitek,mars-sd", sdmmc::init)
});

fn interrupts<'a>(node: &'a FdtNode) -> impl Iterator<Item = u32> + 'a {
    let size = node
        .interrupt_parent()
        .and_then(|ip| ip.interrupt_cells())
        .unwrap_or(1);
    let value = node.property("interrupts").map_or(&[] as _, |p| p.value);
    value
        .chunks(size * mem::size_of::<u32>())
        .map(|v| u32::from_be_bytes(v[..4].try_into().unwrap()))
}

/// Initialize all the possible devices in this crate using FDT.
///
/// # Errors
///
/// This function will return an error if the given base pointer contains an
/// invalid FDT.
///
/// # Safety
///
/// `fdt_base` must have `'static` read access to a valid FDT struct.
pub unsafe fn init(fdt_base: *const ()) -> Result<(), FdtError> {
    static FDT: Once<Fdt> = Once::new();
    let fdt = FDT.try_call_once(|| unsafe { fdt::Fdt::from_ptr(fdt_base.cast()) })?;

    // Some devices may depend on other devices (like interrupts), so we should keep
    // trying until no device get initialized in a turn.

    let mut nodes = fdt.all_nodes().collect::<Vec<_>>();
    let mut count = nodes.len();
    loop {
        if nodes.is_empty() {
            break;
        }

        nodes.retain(|node| {
            if let Some(compat) = node.compatible() {
                let init = compat.all().any(|key| {
                    let ret = DEV_INIT.handle(key, (node, fdt));
                    matches!(ret, Some(true))
                });
                if init {
                    log::debug!("{} initialized", node.name);
                }
                return !init;
            }
            false
        });

        if count == nodes.len() {
            break;
        }
        count = nodes.len();
    }

    Ok(())
}
