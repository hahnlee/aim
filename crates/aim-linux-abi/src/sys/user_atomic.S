// Fault-contained user futex operations. Only user access PCs have fixups;
// faults in the kernel-owned output behave like other host faults.
.text
.p2align 2
.globl _aim_user_atomic
// x0: address, w1: operation (load/store/cmpxchg), w2: expected,
// w3: replacement, x4: owned output. Return 0, 1 (mismatch), or errno.
_aim_user_atomic:
    cbz w1, Latomic_load
    cmp w1, #1
    b.eq Latomic_store
Latomic_ldxr:
    ldaxr w5, [x0]
    cmp w5, w2
    b.ne Latomic_mismatch
Latomic_stxr:
    stlxr w6, w3, [x0]
    cbnz w6, Latomic_ldxr
    str w5, [x4]
    mov w0, #0
    ret
Latomic_load:
    ldar w5, [x0]
    str w5, [x4]
    mov w0, #0
    ret
Latomic_store:
    stlr w3, [x0]
    mov w0, #0
    ret
Latomic_mismatch:
    clrex
    str w5, [x4]
    mov w0, #1
    ret
Latomic_fault:
    clrex
    mov w0, w9
    ret

.section __DATA_CONST,__const
.p2align 3
.globl _aim_user_atomic_ex_table
_aim_user_atomic_ex_table:
    .quad Latomic_ldxr, _aim_user_atomic, Latomic_fault
    .quad Latomic_stxr, _aim_user_atomic, Latomic_fault
    .quad Latomic_load, _aim_user_atomic, Latomic_fault
    .quad Latomic_store, _aim_user_atomic, Latomic_fault
