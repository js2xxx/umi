use devices::{dev::Plic, IntrManager};
use fdt::node::FdtNode;
use rv39_paging::{PAddr, ID_OFFSET};
use spin::{Lazy, Once};

static PLIC: Once<Plic> = Once::new();

pub fn init_plic(fdt: &FdtNode) -> bool {
    let res: Result<&Plic, &str> = PLIC.try_call_once(|| {
        let reg = fdt.reg().and_then(|mut reg| reg.next());
        let reg = reg.ok_or("should have memory registers")?;

        let base = PAddr::new(reg.starting_address as usize);

        // SAFETY: The memory is statically mapped.
        Ok(unsafe { Plic::new(base.to_laddr(ID_OFFSET).as_non_null_unchecked().cast()) })
    });
    res.inspect_err(|err| log::warn!("Skip invalid PLIC: {err}"))
        .is_ok()
}

pub static INTR: Lazy<&IntrManager> = Lazy::new(|| intr_man().expect("PLIC uninitialized"));

pub(in crate::dev) fn intr_man() -> Option<&'static IntrManager> {
    static ONCE: Once<IntrManager> = Once::new();
    ONCE.try_call_once(|| {
        let plic = PLIC.get().cloned();
        plic.map(IntrManager::new).ok_or(())
    })
    .ok()
}
