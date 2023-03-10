use core::arch::global_asm;

global_asm!(
    "
.macro load_regs
    ld x1, 0(a0)
    ld x2, 8(a0)
    ld x5, 32(a0)
    ld x6, 40(a0)
    ld x7, 48(a0)
    ld x8, 56(a0)
    ld x9, 64(a0)
    ld x10, 72(a0)
    ld x11, 80(a0)
    ld x12, 88(a0)
    ld x13, 96(a0)
    ld x14, 104(a0)
    ld x15, 112(a0)
    ld x16, 120(a0)
    ld x17, 128(a0)
    ld x18, 136(a0)
    ld x19, 144(a0)
    ld x20, 152(a0)
    ld x21, 160(a0)
    ld x22, 168(a0)
    ld x23, 176(a0)
    ld x24, 184(a0)
    ld x25, 192(a0)
    ld x26, 200(a0)
    ld x27, 208(a0)
    ld x28, 216(a0)
    ld x29, 224(a0)
    ld x30, 232(a0)
    ld x31, 240(a0)
.endm

.macro save_regs
    sd x1, 0(a0)
    sd x2, 8(a0)
    sd x5, 32(a0)
    sd x6, 40(a0)
    sd x7, 48(a0)
    sd x8, 56(a0)
    sd x9, 64(a0)
    sd x10, 72(a0)
    sd x11, 80(a0)
    sd x12, 88(a0)
    sd x13, 96(a0)
    sd x14, 104(a0)
    sd x15, 112(a0)
    sd x16, 120(a0)
    sd x17, 128(a0)
    sd x18, 136(a0)
    sd x19, 144(a0)
    sd x20, 152(a0)
    sd x21, 160(a0)
    sd x22, 168(a0)
    sd x23, 176(a0)
    sd x24, 184(a0)
    sd x25, 192(a0)
    sd x26, 200(a0)
    sd x27, 208(a0)
    sd x28, 216(a0)
    sd x29, 224(a0)
    sd x30, 232(a0)
    sd x31, 240(a0)
.endm

.global _return_to_user
.type _return_to_user, @function
_return_to_user:
    .cfi_startproc
    sd sp, 256(a0)
    sd ra, 264(a0)

    // Load `a0`
    ld t0, 248(a0)
    csrw sscratch, t0
    // Swap `gp` and `tp`
    ld t0, 16(a0)
    sd gp, 16(a0)
    mv gp, t0
    ld t0, 24(a0)
    sd tp, 24(a0)
    mv tp, t0
    load_regs
    
    csrrw a0, sscratch, a0
    sret
    .cfi_endproc

.global _intr_entry
.type _intr_entry, @function
_intr_entry:
    .cfi_startproc
    csrrw a0, sscratch, a0
    beqz a0, .Lreent

    save_regs
    // Swap `gp` and `tp`
    ld t0, 16(a0)
    sd gp, 16(a0)
    mv gp, t0
    ld t0, 24(a0)
    sd tp, 24(a0)
    mv tp, t0
    // Save `a0`
    csrrw t0, sscratch, zero
    sd t0, 248(a0)

    ld sp, 256(a0)
    ld ra, 264(a0)
    ret
.Lreent:
    csrr a0, sscratch

    save_regs
    csrrw t0, sscratch, zero
    sd t0, 248(a0)

    la t0, {reent_handler}
    ld t0, 0(t0)
    mv s1, a0
    call t0
    mv a0, s1

    ld t0, 248(a0)
    csrw sscratch, t0
    load_regs

    csrrw a0, sscratch, zero
    sret
    .cfi_endproc
",
    reent_handler = sym super::REENT_HANDLER
);
