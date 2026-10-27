// Linux arm64 uaccess / _ASM_EXTABLE model: only user load/store PCs recover.
// x0: source, x1: destination, x2: remaining. Return (remaining, errno) in x0/x1.
.text
.p2align 2
.globl _aim_user_copy
_aim_user_copy:
    cbz x2, Lcopy_done
    cmp x2, #8
    b.lo Lcopy_byte
Lcopy_load8:
    ldr x3, [x0]
Lcopy_store8:
    str x3, [x1]
    add x0, x0, #8
    add x1, x1, #8
    sub x2, x2, #8
    b _aim_user_copy
Lcopy_byte:
Lcopy_load1:
    ldrb w3, [x0]
Lcopy_store1:
    strb w3, [x1]
    add x0, x0, #1
    add x1, x1, #1
    sub x2, x2, #1
    b _aim_user_copy
Lcopy_done:
    mov x0, #0
    mov x1, #0
    ret
Lcopy_fault:
    mov x0, x2
    mov x1, x9
    ret

// Entered only after sigreturn. x7/x8 name the retry/fixup from the table.
// The interrupted leaf has no stack frame; preserve its live arguments and
// return address across pager work on the original host stack.
.globl _aim_user_pager
_aim_user_pager:
    stp x29, x30, [sp, #-96]!
    mov x29, sp
    stp x0, x1, [sp, #16]
    stp x2, x3, [sp, #32]
    stp x4, x5, [sp, #48]
    stp x6, x7, [sp, #64]
    str x8, [sp, #80]
    bl _aim_resolve_user_fault
    mov w9, w0
    ldp x0, x1, [sp, #16]
    ldp x2, x3, [sp, #32]
    ldp x4, x5, [sp, #48]
    ldp x6, x7, [sp, #64]
    ldr x8, [sp, #80]
    ldp x29, x30, [sp], #96
    cbnz w9, Luser_fault
    br x7
Luser_fault:
    br x8

.section __DATA_CONST,__const
.p2align 3
.globl _aim_user_copy_ex_table
_aim_user_copy_ex_table:
    .quad Lcopy_load8, _aim_user_copy, Lcopy_fault
    .quad Lcopy_store8, _aim_user_copy, Lcopy_fault
    .quad Lcopy_load1, _aim_user_copy, Lcopy_fault
    .quad Lcopy_store1, _aim_user_copy, Lcopy_fault
