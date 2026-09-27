// Shared prototype pieces for the P0 experiments: the guest register frame,
// the per-thread host state, and the `svc #0` replacement.
//
// This is a stand-in for what the real loader and syscall layer will do; it
// exists so experiments 2, 4 and 6 exercise the same entry/exit path.
//
// Guest side of a redirected `svc #0` (what a load-time rewriter emits in a
// per-site thunk; here it is the LX_SVC assembler macro below):
//
//     stp  x16, x17, [sp, #-32]!
//     str  x30, [sp, #16]
//     bl   _lx_svc_entry          // x30 = resume point below
//     ldr  x30, [sp, #16]
//     ldp  x16, x17, [sp], #32
//
// lx_svc_entry saves the full guest state into the calling thread's
// lx_thread (found through a Darwin TSD slot, i.e. TPIDRRO_EL0), switches to
// the host stack, and calls lx_dispatch(). lx_resume() goes back to any
// "resume point" (the instruction after the bl above) with every register
// taken from the frame. x16/x17/x30 travel through the 32 bytes below the
// guest sp, which is why the resume point must be a thunk tail. The
// guest-visible pc of a syscall is therefore frame.pc + LX_SVC_TAIL_BYTES.
//
// Caveat found while building this: the thunk writes 32 bytes below the
// guest sp. bionic's _exit_with_stack_teardown munmaps its own stack and
// then issues `exit`, so the syscall layer must defer an munmap that covers
// the calling thread's current sp until that thread exits.
#ifndef P0_LX_H
#define P0_LX_H

#define LX_SVC_TAIL_BYTES 8

// lx_thread.state
#define LX_STATE_GUEST 0     // running guest code
#define LX_STATE_HOST 2      // inside lx_dispatch; frame f is authoritative
#define LX_STATE_EXITING 3   // leaving lx_dispatch; frame f is final
#define LX_STATE_RESUMING 4  // rt_sigreturn: waiting for the resume trap

#ifdef __ASSEMBLER__
    .macro LX_SVC
    stp     x16, x17, [sp, #-32]!
    str     x30, [sp, #16]
    bl      _lx_svc_entry
    ldr     x30, [sp, #16]
    ldp     x16, x17, [sp], #32
    .endm

#define LX_FRAME_X 0
#define LX_FRAME_SP 248
#define LX_FRAME_PC 256
#define LX_FRAME_NZCV 264
#define LX_FRAME_V 272
#define LX_FRAME_FPSR 784
#define LX_FRAME_FPCR 788
#define LX_FRAME_SIZE 800
#define LX_THREAD_HOST_SP 800
#define LX_THREAD_STATE 808
#else
#include <stddef.h>
#include <stdint.h>

struct lx_frame {
  uint64_t x[31];    // x0..x30
  uint64_t sp;       // guest sp at the svc
  uint64_t pc;       // resume point
  uint64_t nzcv;
  __uint128_t v[32];
  uint32_t fpsr, fpcr;
  uint64_t pad;
};
_Static_assert(offsetof(struct lx_frame, sp) == 248, "sp");
_Static_assert(offsetof(struct lx_frame, v) == 272, "v");
_Static_assert(offsetof(struct lx_frame, fpsr) == 784, "fpsr");
_Static_assert(sizeof(struct lx_frame) == 800, "frame size");

struct lx_thread {
  struct lx_frame f;           // must be first
  uint64_t host_sp;            // top of the host stack used by lx_dispatch
  _Atomic uint32_t state;      // LX_STATE_*
  int tid;                     // Linux tid we made up
  uint32_t *clear_child_tid;   // CLONE_CHILD_CLEARTID / set_tid_address
  uint64_t guest_tp;           // guest TPIDR_EL0 value (CLONE_SETTLS)
  void *user;                  // experiment-specific
};

_Static_assert(offsetof(struct lx_thread, state) == 808, "state");

// Offset of our lx_thread* inside the Darwin TSD array (TPIDRRO_EL0 + off).
extern uint64_t lx_host_slot_off;
// Offset of the guest thread pointer (what bionic thinks is TPIDR_EL0).
extern uint64_t lx_guest_tp_off;

void lx_init_process(void);
// Bind the calling host thread to t. host_stack_top is where lx_dispatch runs.
void lx_bind_thread(struct lx_thread *t, uint64_t host_stack_top);
struct lx_thread *lx_self(void);
void lx_set_guest_tp(uint64_t tp);  // what a rewritten `msr tpidr_el0` does

// Assembly.
void lx_svc_entry(void);
__attribute__((noreturn)) void lx_resume(struct lx_frame *f);
extern char lx_svc_entry_end[], lx_resume_end[];
// Implemented by each experiment.
void lx_dispatch(struct lx_thread *t);

// What a rewritten `mrs xN, tpidr_el0` does.
static inline uint64_t lx_read_guest_tp(void) {
  uint64_t v;
  __asm__ volatile("mrs %0, tpidrro_el0\n ldr %0, [%0, %1]" : "=&r"(v) : "r"(lx_guest_tp_off));
  return v;
}
#endif
#endif
