mod block;
mod intr;
mod serial;
mod virtio;

use alloc::vec::Vec;

use fdt::{node::FdtNode, Fdt, FdtError};
use ksc::Handlers;
use spin::{Lazy, Once};

pub use self::{
    block::{block, blocks},
    intr::INTR,
    serial::{stdout, Stdout},
};

static DEV_INIT: Lazy<Handlers<&str, &FdtNode, bool>> = Lazy::new(|| {
    Handlers::new()
        .map("ns16550a", serial::init)
        .map("riscv,plic0", intr::init_plic)
        .map("virtio,mmio", virtio::init_mmio)
});

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
                    let ret = DEV_INIT.handle(key, node);
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
