use fdt::{node::FdtNode, Fdt};
use plic::IntrManager;
use rv39_paging::{PAddr, ID_OFFSET};
use spin::{Lazy, Once};

static PLIC: Once<IntrManager> = Once::new();

pub fn init_plic(fdt: &FdtNode, _: &Fdt) -> bool {
    let res: Result<&IntrManager, &str> = PLIC.try_call_once(|| {
        let reg = fdt.reg().and_then(|mut reg| reg.next());
        let reg = reg.ok_or("should have memory registers")?;

        let base = PAddr::new(reg.starting_address as usize);

        // SAFETY: The memory is statically mapped.
        Ok(unsafe {
            let base = base.to_laddr(ID_OFFSET).as_non_null_unchecked().cast();
            IntrManager::from_raw(base)
        })
    });
    res.inspect_err(|err| log::warn!("Skip invalid PLIC: {err}"))
        .is_ok()
}

pub static INTR: Lazy<&IntrManager> = Lazy::new(|| intr_man().expect("PLIC uninitialized"));

pub fn intr_man() -> Option<&'static IntrManager> {
    PLIC.get()
}
