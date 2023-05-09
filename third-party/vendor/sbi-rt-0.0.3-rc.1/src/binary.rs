//! Capture 3. Binary Encoding

pub use sbi_spec::binary::SbiRet;

#[inline(always)]
pub(crate) fn sbi_call_0(eid: usize, fid: usize) -> SbiRet {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    {
        let (error, value);
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a7") eid,
                in("a6") fid,
                lateout("a0") error,
                lateout("a1") value,
            );
        }
        SbiRet { error, value }
    }
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    {
        let _ = (eid, fid);
        unimplemented!("not RISC-V instruction set architecture")
    }
}

#[inline(always)]
pub(crate) fn sbi_call_1(eid: usize, fid: usize, arg0: usize) -> SbiRet {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    {
        let (error, value);
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a7") eid,
                in("a6") fid,
                inlateout("a0") arg0 => error,
                lateout("a1") value,
            );
        }
        SbiRet { error, value }
    }
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    {
        let _ = (eid, fid, arg0);
        unimplemented!("not RISC-V instruction set architecture")
    }
}

#[inline(always)]
pub(crate) fn sbi_call_2(eid: usize, fid: usize, arg0: usize, arg1: usize) -> SbiRet {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    {
        let (error, value);
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a7") eid,
                in("a6") fid,
                inlateout("a0") arg0 => error,
                inlateout("a1") arg1 => value,
            );
        }
        SbiRet { error, value }
    }
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    {
        let _ = (eid, fid, arg0, arg1);
        unimplemented!("not RISC-V instruction set architecture")
    }
}

#[inline(always)]
pub(crate) fn sbi_call_3(eid: usize, fid: usize, arg0: usize, arg1: usize, arg2: usize) -> SbiRet {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    {
        let (error, value);
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a7") eid,
                in("a6") fid,
                inlateout("a0") arg0 => error,
                inlateout("a1") arg1 => value,
                in("a2") arg2,
            );
        }
        SbiRet { error, value }
    }
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    {
        let _ = (eid, fid, arg0, arg1, arg2);
        unimplemented!("not RISC-V instruction set architecture")
    }
}

#[inline(always)]
pub(crate) fn sbi_call_4(
    eid: usize,
    fid: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
) -> SbiRet {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    {
        let (error, value);
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a7") eid,
                in("a6") fid,
                inlateout("a0") arg0 => error,
                inlateout("a1") arg1 => value,
                in("a2") arg2,
                in("a3") arg3,
            );
        }
        SbiRet { error, value }
    }
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    {
        let _ = (eid, fid, arg0, arg1, arg2, arg3);
        unimplemented!("not RISC-V instruction set architecture")
    }
}

#[inline(always)]
pub(crate) fn sbi_call_5(
    eid: usize,
    fid: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
) -> SbiRet {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    {
        let (error, value);
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a7") eid,
                in("a6") fid,
                inlateout("a0") arg0 => error,
                inlateout("a1") arg1 => value,
                in("a2") arg2,
                in("a3") arg3,
                in("a4") arg4,
            );
        }
        SbiRet { error, value }
    }
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    {
        let _ = (eid, fid, arg0, arg1, arg2, arg3, arg4);
        unimplemented!("not RISC-V instruction set architecture")
    }
}

#[cfg(target_pointer_width = "32")]
#[inline(always)]
pub(crate) fn sbi_call_6(
    eid: usize,
    fid: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> SbiRet {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    {
        let (error, value);
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a7") eid,
                in("a6") fid,
                inlateout("a0") arg0 => error,
                inlateout("a1") arg1 => value,
                in("a2") arg2,
                in("a3") arg3,
                in("a4") arg4,
                in("a5") arg5,
            );
        }
        SbiRet { error, value }
    }
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    {
        let _ = (eid, fid, arg0, arg1, arg2, arg3, arg4, arg5);
        unimplemented!("not RISC-V instruction set architecture")
    }
}
