.extern ktrap_handler

.global ktrap_entry
.type ktrap_entry, @function
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
    addi a0, a0, 4
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
