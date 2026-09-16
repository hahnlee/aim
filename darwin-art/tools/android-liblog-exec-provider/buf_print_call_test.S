.text
.p2align 2
// Test caller using Android AAPCS64, including spilled integer and FP varargs.
.globl _darwin_art_test_log_buf_print
_darwin_art_test_log_buf_print:
  sub sp, sp, #32
  str x30, [sp, #16]
  mov x16, x0
  mov x3, x2
  mov x2, x1
  mov x0, #0
  mov x1, #4
  mov x4, #1
  mov x5, #2
  mov x6, #3
  mov x7, #4
  mov x8, #5
  str x8, [sp]
  fmov d0, #1.5
  blr x16
  ldr x30, [sp, #16]
  add sp, sp, #32
  ret
