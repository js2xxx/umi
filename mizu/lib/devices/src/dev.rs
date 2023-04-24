mod common;
mod plic;
mod virtio_blk;

use fdt::{node::FdtNode, Fdt, FdtError};
use ksc::Handlers;
use spin::{Lazy, Once};

pub use self::{common::*, plic::*, virtio_blk::*};

static DEV_INIT: Lazy<Handlers<&str, &FdtNode, ()>> =
    Lazy::new(|| Handlers::new().map("riscv,plic0", init_plic));

/// Initialize all the possible devices in this crate using FDT.
///
/// # Errors
///
/// This function will return an error if the given base pointer contains a
/// invalid FDT.
///
/// # Safety
///
/// `fdt_base` must have `'static` read access to a valid FDT struct.
pub unsafe fn init(fdt_base: *const ()) -> Result<(), FdtError> {
    static FDT: Once<Fdt> = Once::new();
    let fdt = FDT.try_call_once(|| unsafe { fdt::Fdt::from_ptr(fdt_base.cast()) })?;

    fdt.all_nodes().for_each(|node| {
        log::debug!("Found FDT node: {}", node.name);
        if let Some(compat) = node.compatible() {
            compat.all().for_each(|key| {
                let ret = DEV_INIT.handle(key, &node);
                if ret.is_some() {
                    log::debug!("\tInitialized");
                }
            })
        }
    });
    Ok(())
}
