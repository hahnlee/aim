.text
.p2align 2
.globl _darwin_art_test_log_assert
// Host fixed arguments: address, condition, tag, format.
_darwin_art_test_log_assert:
  sub sp, sp, #32
  str x30, [sp, #16]
  mov x16, x0
  mov x0, x1
  mov x1, x2
  mov x2, x3
  mov x3, #1
  mov x4, #2
  mov x5, #3
  mov x6, #4
  mov x7, #5
  mov x8, #6
  str x8, [sp]
  fmov d0, #1.5
  blr x16
  ldr x30, [sp, #16]
  add sp, sp, #32
  ret
