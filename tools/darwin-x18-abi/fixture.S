.text
.p2align 2
.globl _aim_x18_yield_probe
_aim_x18_yield_probe:
  stp x29, x30, [sp, #-32]!
  stp x19, x20, [sp, #16]
  mov x29, sp
  mov x19, x0
  mov x20, #0x1234
  mov x18, x20
1:
  bl _sched_yield
  cmp x18, x20
  b.ne 2f
  subs x19, x19, #1
  b.ne 1b
  mov w0, #1
  b 3f
2:
  mov w0, #0
3:
  ldp x19, x20, [sp, #16]
  ldp x29, x30, [sp], #32
  ret
