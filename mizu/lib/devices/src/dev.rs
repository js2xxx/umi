mod block;
mod common;
mod plic;
mod virtio_blk;

use alloc::vec::Vec;
use core::mem;

use fdt::{node::FdtNode, Fdt, FdtError};
use ksc::Handlers;
use spin::{Lazy, Once};

pub use self::{common::*, plic::*, virtio_blk::*};

static DEV_INIT: Lazy<Handlers<&str, &FdtNode, bool>> =
    Lazy::new(|| Handlers::new().map("riscv,plic0", init_plic));

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

    let mut storage = [fdt.all_nodes().collect(), Vec::new()];
    let mut rep = 0;
    let mut count = storage[rep].len();
    loop {
        let nodes = mem::take(&mut storage[rep]);
        let next_rep = 1 - rep;
        let next_nodes = &mut storage[next_rep];

        if nodes.is_empty() {
            break;
        }

        nodes.into_iter().for_each(|node| {
            if let Some(compat) = node.compatible() {
                let init = compat.all().any(|key| {
                    let ret = DEV_INIT.handle(key, &node);
                    matches!(ret, Some(true))
                });
                if init {
                    log::debug!("{} initialized", node.name);
                } else {
                    next_nodes.push(node)
                }
            }
        });

        if count == next_nodes.len() {
            break;
        }
        count = next_nodes.len();
        rep = next_rep;
    }

    Ok(())
}
