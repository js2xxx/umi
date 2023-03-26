// a0 <- trap_frame: *mut TrapFrame
// a1 <- scratch register

.macro scratch op
    \op a1, 17*8(a0)
.endm

.macro save_scratch
    scratch sd
.endm

.macro load_scratch
    scratch ld
.endm

.macro xchg reg addr
    ld a1, \addr
    sd \reg, \addr
    mv \reg, a1
.endm

.macro xchg_sx
    xchg s0, 0*8(a0)
    xchg s1, 1*8(a0)
    xchg s2, 2*8(a0)
    xchg s3, 3*8(a0)
    xchg s4, 4*8(a0)
    xchg s5, 5*8(a0)
    xchg s6, 6*8(a0)
    xchg s7, 7*8(a0)
    xchg s8, 8*8(a0)
    xchg s9, 9*8(a0)
    xchg s10, 10*8(a0)
    xchg s11, 11*8(a0)
.endm

.macro tx op
    xchg ra, 12*8(a0)
    xchg sp, 13*8(a0)
    xchg gp, 14*8(a0)
    xchg tp, 15*8(a0)
    \op a2, 18*8(a0)
    \op a3, 19*8(a0)
    \op a4, 20*8(a0)
    \op a5, 21*8(a0)
    \op a6, 22*8(a0)
    \op a7, 23*8(a0)
    \op t0, 24*8(a0)
    \op t1, 25*8(a0)
    \op t2, 26*8(a0)
    \op t3, 27*8(a0)
    \op t4, 28*8(a0)
    \op t5, 29*8(a0)
    \op t6, 30*8(a0)
.endm

.macro save_tx
    tx sd
.endm

.macro load_tx
    tx ld
.endm

.macro save_ux 
    csrr a1, sepc
    sd a1, 31*8(a0)
    csrr a1, sstatus
    sd a1, 32*8(a0)
    csrr a1, stval
    sd a1, 33*8(a0)
    csrr a1, scause
    sd a1, 34*8(a0)
.endm

.macro load_ux 
    ld a1, 31*8(a0)
    csrw sepc, a1
    ld a1, 32*8(a0)
    csrw sstatus, a1
.endm

.section .text

.global _return_to_user
.type _return_to_user, @function
_return_to_user:
    xchg_sx

.global _fast_return_to_user
.type _fast_return_to_user, @function
_fast_return_to_user:
    load_ux
    load_tx
    load_scratch

    csrw sscratch, a0
    ld a0, 16*8(a0)
    sret

.extern _fast_func

.global _user_entry
.type _user_entry, @function
_user_entry:
    csrrw a0, sscratch, a0
    save_scratch
    save_tx
    save_ux
    csrr a1, sscratch
    sd a1, 16*8(a0)

    call _fast_func
    // `a0` did not change;
    // `a1` <- status;
    bnez a1, .Lresume
    j _fast_return_to_user

.Lresume:
    mv t0, a1
    xchg_sx
    mv a0, t0
    ret
