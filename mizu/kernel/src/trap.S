.extern ktrap_handler

.global ktrap_entry
.type ktrap_entry, @function
.align 4
ktrap_entry:
    addi sp, sp, -20*8

    sd ra, 1*8(sp)
    sd sp, 2*8(sp) // Should be added 16 in the dump
    sd gp, 3*8(sp)
    sd tp, 4*8(sp)
    sd a0, 5*8(sp)
    sd a1, 6*8(sp)
    sd a2, 7*8(sp)
    sd a3, 8*8(sp)
    sd a4, 9*8(sp)
    sd a5, 10*8(sp)
    sd a6, 11*8(sp)
    sd a7, 12*8(sp)
    sd t0, 13*8(sp)
    sd t1, 14*8(sp)
    sd t2, 15*8(sp)
    sd t3, 16*8(sp)
    sd t4, 17*8(sp)
    sd t5, 18*8(sp)
    sd t6, 19*8(sp)

    mv a0, sp
    addi a0, a0, 8
    call ktrap_handler

    ld ra, 1*8(sp)
    // Ignore sp
    ld gp, 3*8(sp)
    ld tp, 4*8(sp)
    ld a0, 5*8(sp)
    ld a1, 6*8(sp)
    ld a2, 7*8(sp)
    ld a3, 8*8(sp)
    ld a4, 9*8(sp)
    ld a5, 10*8(sp)
    ld a6, 11*8(sp)
    ld a7, 12*8(sp)
    ld t0, 13*8(sp)
    ld t1, 14*8(sp)
    ld t2, 15*8(sp)
    ld t3, 16*8(sp)
    ld t4, 17*8(sp)
    ld t5, 18*8(sp)
    ld t6, 19*8(sp)

    addi sp, sp, 20*8
    sret

.global _checked_copy
.type _checked_copy, @function
_checked_copy:
.Lcopy_loop:
    beqz a2, .Lcopy_ret
    lb t0, 0(a0)
    sb t0, 0(a1)
    addi a0, a0, 1
    addi a1, a1, 1
    addi a2, a2, -1
    j .Lcopy_loop
.Lcopy_ret:
    li a0, 0
    ret

.global _checked_zero
.type _checked_zero, @function
_checked_zero:
    mv t0, a0
.Lzero_loop:
    beqz a2, .Lzero_ret
    sb t0, 0(a1)
    addi a0, a0, 1
    addi a1, a1, 1
    addi a2, a2, -1
    j .Lzero_loop
.Lzero_ret:
    li a0, 0
    ret

.global _checked_load_u32
.type _checked_load_u32, @function
_checked_load_u32:
    lw t0, 0(a0)
    fence r, rw
    sd t0, 0(a1)
    li a0, 0
    ret

.global _checked_ua_fault
.type _checked_ua_fault, @function
_checked_ua_fault:
    ret
