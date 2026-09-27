// Dispatcher for the lean entry paths. Built with -mgeneral-regs-only so
// the compiler cannot touch v0-v31, which is what makes skipping the SIMD
// save legal.
#include "lx.h"

void lean_dispatch(struct lx_thread *t) {
  struct lx_frame *f = &t->f;
  switch (f->x[8]) {
    case 178: f->x[0] = (uint64_t)t->tid; break;  // gettid
    default: f->x[0] = (uint64_t)-38;             // -ENOSYS
  }
}
