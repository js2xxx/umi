use core::arch::global_asm;

global_asm!(
    r#"
.macro load_regs reg
    ld t0, 256(\reg)
    ld t1, 264(\reg)
    csrw sepc, t0
    csrw sstatus, t1
    ld x1, 0(\reg)
    ld x2, 8(\reg)
    ld x5, 32(\reg)
    ld x6, 40(\reg)
    ld x7, 48(\reg)
    ld x8, 56(\reg)
    ld x9, 64(\reg)
    ld x10, 72(\reg)
    ld x11, 80(\reg)
    ld x12, 88(\reg)
    ld x13, 96(\reg)
    ld x14, 104(\reg)
    ld x15, 112(\reg)
    ld x16, 120(\reg)
    ld x17, 128(\reg)
    ld x18, 136(\reg)
    ld x19, 144(\reg)
    ld x20, 152(\reg)
    ld x21, 160(\reg)
    ld x22, 168(\reg)
    ld x23, 176(\reg)
    ld x24, 184(\reg)
    ld x25, 192(\reg)
    ld x26, 200(\reg)
    ld x27, 208(\reg)
    ld x28, 216(\reg)
    ld x29, 224(\reg)
    ld x30, 232(\reg)
    ld x31, 240(\reg)
.endm

.macro save_regs reg
    sd x1, 0(\reg)
    sd x2, 8(\reg)
    sd x5, 32(\reg)
    sd x6, 40(\reg)
    sd x7, 48(\reg)
    sd x8, 56(\reg)
    sd x9, 64(\reg)
    sd x10, 72(\reg)
    sd x11, 80(\reg)
    sd x12, 88(\reg)
    sd x13, 96(\reg)
    sd x14, 104(\reg)
    sd x15, 112(\reg)
    sd x16, 120(\reg)
    sd x17, 128(\reg)
    sd x18, 136(\reg)
    sd x19, 144(\reg)
    sd x20, 152(\reg)
    sd x21, 160(\reg)
    sd x22, 168(\reg)
    sd x23, 176(\reg)
    sd x24, 184(\reg)
    sd x25, 192(\reg)
    sd x26, 200(\reg)
    sd x27, 208(\reg)
    sd x28, 216(\reg)
    sd x29, 224(\reg)
    sd x30, 232(\reg)
    sd x31, 240(\reg)
    csrr t0, sepc
    csrr t1, sstatus
    sd t0, 256(\reg)
    sd t1, 264(\reg)
.endm

.global _return_to_user
.type _return_to_user, @function
_return_to_user:
    .cfi_startproc
    sd sp, 272(a0)
    sd ra, 280(a0)

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

    load_regs a0
    
    csrrw a0, sscratch, a0
    sret
    .cfi_endproc

.global _intr_entry
.type _intr_entry, @function
_intr_entry:
    .cfi_startproc
    csrrw a0, sscratch, a0
    beqz a0, .Lreent

    save_regs a0

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

    ld sp, 272(a0)
    ld ra, 280(a0)
    ret
.Lreent:
    csrrw a0, sscratch, zero

    add sp, sp, -288
    sd a0, 248(sp)
    save_regs sp

    la t0, {reent_handler}
    ld t0, 0(t0)
    mv s1, sp
    call t0
    mv sp, s1

    load_regs sp
    ld a0, 248(sp)
    add sp, sp, 288

    sret
    .cfi_endproc
"#,
    reent_handler = sym super::REENT_HANDLER
);
