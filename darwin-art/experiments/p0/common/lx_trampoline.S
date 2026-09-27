// Host side of a redirected `svc #0`: full save, host stack switch,
// dispatch, full restore. See lx.h for the calling convention.
#include "lx.h"

    .text
    .p2align 4
    .globl _lx_svc_entry
_lx_svc_entry:
    // [sp]=guest x16, [sp+8]=guest x17, [sp+16]=guest x30; x30 = resume pc.
    mrs     x16, tpidrro_el0
    adrp    x17, _lx_host_slot_off@PAGE
    ldr     x17, [x17, _lx_host_slot_off@PAGEOFF]
    ldr     x16, [x16, x17]              // struct lx_thread *
    str     x0, [x16, #0]
    mov     w0, #LX_STATE_HOST
    str     w0, [x16, #LX_THREAD_STATE]  // from here on the frame is authoritative
    ldr     x0, [x16, #0]
    stp     x0, x1, [x16, #0]
    stp     x2, x3, [x16, #16]
    stp     x4, x5, [x16, #32]
    stp     x6, x7, [x16, #48]
    stp     x8, x9, [x16, #64]
    stp     x10, x11, [x16, #80]
    stp     x12, x13, [x16, #96]
    stp     x14, x15, [x16, #112]
    ldp     x0, x1, [sp]
    stp     x0, x1, [x16, #128]          // guest x16, x17
    stp     x18, x19, [x16, #144]
    stp     x20, x21, [x16, #160]
    stp     x22, x23, [x16, #176]
    stp     x24, x25, [x16, #192]
    stp     x26, x27, [x16, #208]
    stp     x28, x29, [x16, #224]
    ldr     x0, [sp, #16]
    str     x0, [x16, #240]              // guest x30
    add     x0, sp, #32
    stp     x0, x30, [x16, #LX_FRAME_SP] // guest sp, resume pc
    mrs     x0, nzcv
    str     x0, [x16, #LX_FRAME_NZCV]
    add     x1, x16, #LX_FRAME_V
    stp     q0, q1, [x1, #0]
    stp     q2, q3, [x1, #32]
    stp     q4, q5, [x1, #64]
    stp     q6, q7, [x1, #96]
    stp     q8, q9, [x1, #128]
    stp     q10, q11, [x1, #160]
    stp     q12, q13, [x1, #192]
    stp     q14, q15, [x1, #224]
    stp     q16, q17, [x1, #256]
    stp     q18, q19, [x1, #288]
    stp     q20, q21, [x1, #320]
    stp     q22, q23, [x1, #352]
    stp     q24, q25, [x1, #384]
    stp     q26, q27, [x1, #416]
    stp     q28, q29, [x1, #448]
    stp     q30, q31, [x1, #480]
    mrs     x0, fpsr
    mrs     x2, fpcr
    str     w0, [x16, #LX_FRAME_FPSR]
    str     w2, [x16, #LX_FRAME_FPCR]
    ldr     x0, [x16, #LX_THREAD_HOST_SP]
    mov     sp, x0                       // host stack
    mov     x0, x16
    mov     x29, #0                      // host unwinders stop here
    .globl _lx_svc_entry_end
_lx_svc_entry_end:
    bl      _lx_dispatch
    // lx_dispatch returns to resume the (possibly modified) frame of the
    // calling thread.
    mrs     x16, tpidrro_el0
    adrp    x17, _lx_host_slot_off@PAGE
    ldr     x17, [x17, _lx_host_slot_off@PAGEOFF]
    ldr     x0, [x16, x17]
    // fall through

    .globl _lx_resume
_lx_resume:
    mov     x16, x0
    str     wzr, [x16, #LX_THREAD_STATE] // LX_STATE_GUEST; pc-range covers the rest
    ldr     x0, [x16, #LX_FRAME_SP]
    sub     x0, x0, #32
    mov     sp, x0
    ldp     x0, x1, [x16, #128]
    stp     x0, x1, [sp]                 // guest x16, x17 for the thunk tail
    ldr     x0, [x16, #240]
    str     x0, [sp, #16]                // guest x30
    ldr     x0, [x16, #LX_FRAME_NZCV]
    msr     nzcv, x0
    ldr     w0, [x16, #LX_FRAME_FPSR]
    ldr     w2, [x16, #LX_FRAME_FPCR]
    msr     fpsr, x0
    msr     fpcr, x2
    add     x1, x16, #LX_FRAME_V
    ldp     q0, q1, [x1, #0]
    ldp     q2, q3, [x1, #32]
    ldp     q4, q5, [x1, #64]
    ldp     q6, q7, [x1, #96]
    ldp     q8, q9, [x1, #128]
    ldp     q10, q11, [x1, #160]
    ldp     q12, q13, [x1, #192]
    ldp     q14, q15, [x1, #224]
    ldp     q16, q17, [x1, #256]
    ldp     q18, q19, [x1, #288]
    ldp     q20, q21, [x1, #320]
    ldp     q22, q23, [x1, #352]
    ldp     q24, q25, [x1, #384]
    ldp     q26, q27, [x1, #416]
    ldp     q28, q29, [x1, #448]
    ldp     q30, q31, [x1, #480]
    ldp     x0, x1, [x16, #0]
    ldp     x2, x3, [x16, #16]
    ldp     x4, x5, [x16, #32]
    ldp     x6, x7, [x16, #48]
    ldp     x8, x9, [x16, #64]
    ldp     x10, x11, [x16, #80]
    ldp     x12, x13, [x16, #96]
    ldp     x14, x15, [x16, #112]
    ldp     x18, x19, [x16, #144]
    ldp     x20, x21, [x16, #160]
    ldp     x22, x23, [x16, #176]
    ldp     x24, x25, [x16, #192]
    ldp     x26, x27, [x16, #208]
    ldp     x28, x29, [x16, #224]
    ldr     x16, [x16, #LX_FRAME_PC]
    br      x16                          // thunk tail reloads x16/x17/x30
    .globl _lx_resume_end
_lx_resume_end:
