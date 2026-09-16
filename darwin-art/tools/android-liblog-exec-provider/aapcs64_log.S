.text
.p2align 2

// Three fixed arguments; Android variadic arguments start in x3 / v0.
.globl _darwin_art_android_log_assert
_darwin_art_android_log_assert:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  add x3, sp, #0
  add x4, sp, #64
  add x5, sp, #224
  bl _darwin_art_android_log_assert_captured
  brk #0 // AOSP assert must never return.

.p2align 2

// Capture Android AAPCS64 varargs before crossing into Darwin C. x0-x2 are
// priority, tag, and format; guest formatting arguments begin in x3.
.globl _darwin_art_android_log_print
_darwin_art_android_log_print:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  ldr x2, [sp, #16]
  add x3, sp, #0
  add x4, sp, #64
  add x5, sp, #224
  bl _darwin_art_android_log_print_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret

// buf_print has four fixed arguments, with guest varargs starting in x4.
.p2align 2
.globl _darwin_art_android_log_buf_print
_darwin_art_android_log_buf_print:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  add x4, sp, #0
  add x5, sp, #64
  add x6, sp, #224
  bl _darwin_art_android_log_buf_print_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret
