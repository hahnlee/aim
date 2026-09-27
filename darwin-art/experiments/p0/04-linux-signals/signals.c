// P0 experiment 4: Linux signal delivery with Linux arm64 contexts.
//
// Host side (this file, "lx"):
//   * Every host thread has a host sigaltstack; the host handler for
//     SIGSEGV/SIGBUS/SIGILL/SIGTRAP/SIGFPE (synchronous faults) and SIGUSR2
//     (the carrier for every asynchronous guest signal) runs there.
//   * Asynchronous guest signals (tgkill, and anything darwin-artd routes)
//     are queued per thread with a full Linux siginfo and the target is
//     poked with SIGUSR2. That also gives us Linux RT signals 32..64, which
//     Darwin does not have.
//   * Delivery builds a Linux rt_sigframe (siginfo + ucontext with
//     sigcontext, fpsimd_context, esr_context) on the guest stack or guest
//     sigaltstack, then points the interrupted Darwin context at the guest
//     handler; the host handler returns and Darwin's own sigreturn performs
//     the switch.
//   * rt_sigreturn (from bionic's __restore_rt through the svc thunk) reads
//     the Linux frame, then executes a udf "resume trap" whose SIGILL
//     handler loads the full Linux context into the Darwin mcontext.
//
// Guest side: guest_sig.S (asm probes) and guest_handler() below, written
// against the Linux uapi structs the way ART's fault handler is.
#include "../common/lx.h"

#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <os/os_sync_wait_on_address.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/ucontext.h>
#include <time.h>
#include <unistd.h>

// ------------------------------------------------------ Linux uapi (arm64)
#define LX_SIGQUIT 3
#define LX_SIGILL 4
#define LX_SIGTRAP 5
#define LX_SIGBUS 7
#define LX_SIGFPE 8
#define LX_SIGUSR1 10
#define LX_SIGSEGV 11
#define LX_NSIG 64
#define LX_SA_SIGINFO 0x00000004u
#define LX_SA_RESTORER 0x04000000u
#define LX_SA_ONSTACK 0x08000000u
#define LX_SA_RESTART 0x10000000u
#define LX_SA_NODEFER 0x40000000u
#define LX_SS_ONSTACK 1
#define LX_SS_DISABLE 2
#define LX_SEGV_MAPERR 1
#define LX_SEGV_ACCERR 2
#define LX_BUS_ADRALN 1
#define LX_BUS_ADRERR 2
#define LX_SI_TKILL (-6)
#define LX_SIG_BLOCK 0
#define LX_SIG_UNBLOCK 1
#define LX_SIG_SETMASK 2
#define FPSIMD_MAGIC 0x46508001u
#define ESR_MAGIC 0x45535201u

typedef struct {
  int si_signo, si_errno, si_code, _pad;
  union {
    struct { uint64_t addr; } fault;
    struct { int32_t pid; uint32_t uid; } kill;
    uint8_t raw[112];
  };
} lx_siginfo_t;
typedef struct { uint64_t ss_sp; int32_t ss_flags; int32_t _pad; uint64_t ss_size; } lx_stack_t;
struct lx_sigcontext {
  uint64_t fault_address;
  uint64_t regs[31];
  uint64_t sp, pc, pstate;
  _Alignas(16) uint8_t __reserved[4096];
};
struct _aarch64_ctx { uint32_t magic, size; };
struct fpsimd_context { struct _aarch64_ctx head; uint32_t fpsr, fpcr; __uint128_t vregs[32]; };
struct esr_context { struct _aarch64_ctx head; uint64_t esr; };
typedef struct lx_ucontext {
  uint64_t uc_flags;
  struct lx_ucontext *uc_link;
  lx_stack_t uc_stack;
  uint64_t uc_sigmask;
  uint8_t unused_[128 - sizeof(uint64_t)];  // __unused in the uapi header
  struct lx_sigcontext uc_mcontext;
} lx_ucontext_t;
struct lx_rt_sigframe { lx_siginfo_t info; lx_ucontext_t uc; };
struct lx_frame_record { uint64_t fp, lr; };
struct lx_k_sigaction { uint64_t handler; uint64_t flags; uint64_t restorer; uint64_t mask; };
_Static_assert(sizeof(lx_siginfo_t) == 128, "siginfo");
_Static_assert(offsetof(lx_ucontext_t, uc_mcontext) == 176, "uc_mcontext offset (bionic/Linux arm64)");
_Static_assert(offsetof(struct lx_sigcontext, __reserved) == 288, "__reserved");
_Static_assert(sizeof(struct fpsimd_context) == 528, "fpsimd_context");

// ------------------------------------------------------------ host state
struct lx_cpu {
  uint64_t x[31], sp, pc, pstate;
  __uint128_t v[32];
  uint32_t fpsr, fpcr;
};
struct sigthread {
  struct lx_thread t;              // must be first
  pthread_t pth;
  uint64_t guest_mask;             // Linux numbering, bit (sig-1)
  _Atomic uint64_t pending;
  lx_siginfo_t pinfo[LX_NSIG + 1];
  lx_stack_t altstack;             // guest sigaltstack
  struct lx_cpu resume_cpu;        // rt_sigreturn target
  uint64_t resume_mask;
  _Atomic uint64_t host_signals, delivered;
};
static struct lx_k_sigaction g_act[LX_NSIG + 1];

extern char guest_restore_rt[], lx_enter_handler[], lx_enter_handler_end[], lx_resume_trap[];
void lx_enter_handler_c(const uint64_t e[7]) __asm__("_lx_enter_handler") __attribute__((noreturn));

static inline uint64_t now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }
static inline uint64_t sigbit(int s) { return 1ull << (s - 1); }

static void cpu_from_darwin(struct lx_cpu *c, const struct __darwin_mcontext64 *m) {
  for (int i = 0; i < 29; i++) c->x[i] = m->__ss.__x[i];
  c->x[29] = m->__ss.__fp;
  c->x[30] = m->__ss.__lr;
  c->sp = m->__ss.__sp;
  c->pc = m->__ss.__pc;
  c->pstate = m->__ss.__cpsr;
  memcpy(c->v, m->__ns.__v, sizeof c->v);
  c->fpsr = m->__ns.__fpsr;
  c->fpcr = m->__ns.__fpcr;
}
static void cpu_to_darwin(struct __darwin_mcontext64 *m, const struct lx_cpu *c) {
  for (int i = 0; i < 29; i++) m->__ss.__x[i] = c->x[i];
  m->__ss.__fp = c->x[29];
  m->__ss.__lr = c->x[30];
  m->__ss.__sp = c->sp;
  m->__ss.__pc = c->pc;
  m->__ss.__cpsr = (m->__ss.__cpsr & ~0xf0000000u) | ((uint32_t)c->pstate & 0xf0000000u);  // NZCV only
  memcpy(m->__ns.__v, c->v, sizeof c->v);
  m->__ns.__fpsr = c->fpsr;
  m->__ns.__fpcr = c->fpcr;
}
// Guest-visible state of a thread sitting in a syscall.
static void cpu_from_frame(struct lx_cpu *c, const struct lx_frame *f) {
  memcpy(c->x, f->x, sizeof c->x);
  c->sp = f->sp;
  c->pc = f->pc + LX_SVC_TAIL_BYTES;  // the instruction after the svc
  c->pstate = f->nzcv;
  memcpy(c->v, f->v, sizeof c->v);
  c->fpsr = f->fpsr;
  c->fpcr = f->fpcr;
}

// Build the Linux rt_sigframe exactly as arch/arm64/kernel/signal.c lays it
// out (frame record above the sigframe, 16-byte aligned) and return the
// register values the guest handler starts with: pc, sp, x0, x1, x2, x29, x30.
static void setup_rt_frame(struct sigthread *st, int sig, const lx_siginfo_t *info, const struct lx_cpu *c,
                           uint64_t esr, uint64_t entry[7]) {
  const struct lx_k_sigaction *ka = &g_act[sig];
  uint64_t sp = c->sp;
  int on_alt = st->altstack.ss_size && c->sp - st->altstack.ss_sp < st->altstack.ss_size;
  if ((ka->flags & LX_SA_ONSTACK) && st->altstack.ss_size && !on_alt) sp = st->altstack.ss_sp + st->altstack.ss_size;
  sp = (sp - sizeof(struct lx_frame_record)) & ~15ull;
  struct lx_frame_record *rec = (void *)sp;
  sp = (sp - sizeof(struct lx_rt_sigframe)) & ~15ull;
  struct lx_rt_sigframe *fr = (void *)sp;

  memset(fr, 0, offsetof(struct lx_rt_sigframe, uc.uc_mcontext.__reserved) + sizeof(struct fpsimd_context) +
                    sizeof(struct esr_context) + sizeof(struct _aarch64_ctx));
  fr->info = *info;
  fr->uc.uc_stack.ss_sp = st->altstack.ss_sp;
  fr->uc.uc_stack.ss_size = st->altstack.ss_size;
  fr->uc.uc_stack.ss_flags = st->altstack.ss_size ? (on_alt ? LX_SS_ONSTACK : 0) : LX_SS_DISABLE;
  fr->uc.uc_sigmask = st->guest_mask;
  struct lx_sigcontext *sc = &fr->uc.uc_mcontext;
  sc->fault_address = (sig == LX_SIGSEGV || sig == LX_SIGBUS) ? info->fault.addr : 0;
  memcpy(sc->regs, c->x, sizeof sc->regs);
  sc->sp = c->sp;
  sc->pc = c->pc;
  sc->pstate = c->pstate;
  uint8_t *p = sc->__reserved;
  struct fpsimd_context *fp = (void *)p;
  fp->head.magic = FPSIMD_MAGIC;
  fp->head.size = sizeof *fp;
  fp->fpsr = c->fpsr;
  fp->fpcr = c->fpcr;
  memcpy(fp->vregs, c->v, sizeof fp->vregs);
  p += sizeof *fp;
  if (esr) {
    struct esr_context *e = (void *)p;
    e->head.magic = ESR_MAGIC;
    e->head.size = sizeof *e;
    e->esr = esr;
    p += sizeof *e;
  }
  ((struct _aarch64_ctx *)p)->magic = 0;  // terminator
  ((struct _aarch64_ctx *)p)->size = 0;
  rec->fp = c->x[29];
  rec->lr = c->x[30];

  st->guest_mask |= ka->mask | ((ka->flags & LX_SA_NODEFER) ? 0 : sigbit(sig));
  entry[0] = ka->handler;
  entry[1] = sp;
  entry[2] = (uint64_t)sig;
  entry[3] = (uint64_t)&fr->info;
  entry[4] = (uint64_t)&fr->uc;
  entry[5] = (uint64_t)&rec->fp;
  entry[6] = (ka->flags & LX_SA_RESTORER) ? ka->restorer : (uint64_t)guest_restore_rt;
  atomic_fetch_add(&st->delivered, 1);
}

static void entry_to_darwin(struct __darwin_mcontext64 *m, const uint64_t e[7]) {
  m->__ss.__pc = e[0];
  m->__ss.__sp = e[1];
  m->__ss.__x[0] = e[2];
  m->__ss.__x[1] = e[3];
  m->__ss.__x[2] = e[4];
  m->__ss.__fp = e[5];
  m->__ss.__lr = e[6];
}

static int take_deliverable(struct sigthread *st, lx_siginfo_t *out) {
  uint64_t p = atomic_load(&st->pending) & ~st->guest_mask;
  if (!p) return 0;
  int sig = __builtin_ctzll(p) + 1;
  *out = st->pinfo[sig];
  atomic_fetch_and(&st->pending, ~sigbit(sig));
  return sig;
}

// Translate a Darwin synchronous fault into Linux signo/si_code.
//
// Measured (see README): Darwin raises SIGBUS, not SIGSEGV, for a PROT_NONE
// page, for a write to a read-only page, and for a read past EOF of a
// mapped file, and all three carry the same ESR (data abort, translation
// fault level 3). Neither the signal nor ESR tells them apart, so the
// translation asks the VM map: no mapping -> SEGV_MAPERR; a mapping whose
// protection forbids the access -> SEGV_ACCERR; a mapping that allows it
// -> the pager failed -> SIGBUS/BUS_ADRERR. The real syscall layer can use
// its own guest mapping table instead of mach_vm_region.
static int translate_fault(int hsig, const siginfo_t *si, const struct __darwin_mcontext64 *m, lx_siginfo_t *li) {
  memset(li, 0, sizeof *li);
  uint32_t esr = m->__es.__esr;
  uint32_t ec = esr >> 26, fsc = esr & 0x3f;
  int data_abort = ec == 0x24 || ec == 0x25, insn_abort = ec == 0x20 || ec == 0x21;
  li->fault.addr = data_abort ? m->__es.__far : (uint64_t)si->si_addr;
  if (hsig == SIGSEGV || hsig == SIGBUS) {
    if (data_abort && fsc == 0x21) {
      li->si_signo = LX_SIGBUS; li->si_code = LX_BUS_ADRALN;
      return li->si_signo;
    }
    mach_vm_address_t a = li->fault.addr;
    mach_vm_size_t size = 0;
    vm_region_basic_info_data_64_t info;
    mach_msg_type_number_t cnt = VM_REGION_BASIC_INFO_COUNT_64;
    mach_port_t obj = MACH_PORT_NULL;
    kern_return_t kr = mach_vm_region(mach_task_self(), &a, &size, VM_REGION_BASIC_INFO_64, (vm_region_info_t)&info, &cnt, &obj);
    int mapped = kr == KERN_SUCCESS && a <= li->fault.addr && li->fault.addr - a < size;
    if (!mapped) {
      li->si_signo = LX_SIGSEGV; li->si_code = LX_SEGV_MAPERR;
    } else {
      vm_prot_t need = insn_abort ? VM_PROT_EXECUTE : (data_abort && (esr & (1u << 6))) ? VM_PROT_WRITE : VM_PROT_READ;
      if ((info.protection & need) != need) {
        li->si_signo = LX_SIGSEGV; li->si_code = LX_SEGV_ACCERR;
      } else {
        li->si_signo = LX_SIGBUS; li->si_code = LX_BUS_ADRERR;
      }
    }
  } else if (hsig == SIGILL) {
    li->si_signo = LX_SIGILL; li->si_code = 1; li->fault.addr = m->__ss.__pc;
  } else if (hsig == SIGTRAP) {
    li->si_signo = LX_SIGTRAP; li->si_code = 1; li->fault.addr = m->__ss.__pc;
  } else {
    li->si_signo = LX_SIGFPE; li->si_code = si->si_code; li->fault.addr = m->__ss.__pc;
  }
  return li->si_signo;
}

static _Atomic uint64_t g_raw_log_n;
static struct { int hsig, hcode; uint64_t addr, far, esr; } g_raw_log[16];

static void host_handler(int hsig, siginfo_t *si, void *ucv) {
  ucontext_t *uc = ucv;
  struct __darwin_mcontext64 *m = uc->uc_mcontext;
  struct sigthread *st = (struct sigthread *)lx_self();
  uint64_t pc = m->__ss.__pc;
  if (!st) abort();
  atomic_fetch_add(&st->host_signals, 1);
  uint32_t state = atomic_load(&st->t.state);
  uint64_t e[7];
  lx_siginfo_t li;

  // 1. rt_sigreturn's resume trap.
  if (hsig == SIGILL && pc == (uint64_t)lx_resume_trap && state == LX_STATE_RESUMING) {
    st->guest_mask = st->resume_mask;
    atomic_store(&st->t.state, LX_STATE_GUEST);
    int sig = take_deliverable(st, &li);
    cpu_to_darwin(m, &st->resume_cpu);
    if (sig) {  // something became deliverable when the mask was restored
      setup_rt_frame(st, sig, &li, &st->resume_cpu, 0, e);
      entry_to_darwin(m, e);
    }
    return;
  }

  int in_resume = pc >= (uint64_t)lx_resume && pc < (uint64_t)lx_resume_end;
  int in_entry = pc >= (uint64_t)lx_svc_entry && pc < (uint64_t)lx_svc_entry_end;
  int in_enter = pc >= (uint64_t)lx_enter_handler && pc < (uint64_t)lx_enter_handler_end;

  if (hsig == SIGUSR2) {  // carrier for queued asynchronous guest signals
    if (in_entry || in_enter || state == LX_STATE_HOST || state == LX_STATE_RESUMING) return;  // stays pending
    int sig = take_deliverable(st, &li);
    if (!sig) return;
    struct lx_cpu c;
    if (in_resume || state == LX_STATE_EXITING) cpu_from_frame(&c, &st->t.f);  // frame is final
    else cpu_from_darwin(&c, m);
    atomic_store(&st->t.state, LX_STATE_GUEST);
    setup_rt_frame(st, sig, &li, &c, 0, e);
    entry_to_darwin(m, e);
    return;
  }

  // Synchronous fault.
  uint64_t n = atomic_fetch_add(&g_raw_log_n, 1);
  if (n < 16) {
    g_raw_log[n].hsig = hsig;
    g_raw_log[n].hcode = si->si_code;
    g_raw_log[n].addr = (uint64_t)si->si_addr;
    g_raw_log[n].far = m->__es.__far;
    g_raw_log[n].esr = m->__es.__esr;
  }
  if (state != LX_STATE_GUEST || in_resume || in_entry || in_enter) {
    // A fault inside the syscall layer: in the real thing this is an
    // EFAULT path (guest pointer) or a host bug.
    fprintf(stderr, "host fault: sig %d at pc %#llx addr %p state %u\n", hsig, pc, si->si_addr, state);
    abort();
  }
  int sig = translate_fault(hsig, si, m, &li);
  if (!g_act[sig].handler) {
    fprintf(stderr, "guest signal %d without handler: default action (terminate)\n", sig);
    _exit(128 + sig);
  }
  struct lx_cpu c;
  cpu_from_darwin(&c, m);
  setup_rt_frame(st, sig, &li, &c, m->__es.__esr, e);
  entry_to_darwin(m, e);
}

static void host_thread_signals(void) {
  stack_t ss = {.ss_sp = malloc(256 * 1024), .ss_size = 256 * 1024};
  sigaltstack(&ss, NULL);
}

static void install_host_handlers(void) {
  struct sigaction sa = {0};
  sa.sa_sigaction = host_handler;
  sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
  sigfillset(&sa.sa_mask);
  int sigs[] = {SIGSEGV, SIGBUS, SIGILL, SIGTRAP, SIGFPE, SIGUSR2};
  for (unsigned i = 0; i < sizeof sigs / sizeof sigs[0]; i++) sigaction(sigs[i], &sa, NULL);
}

// Exit path of lx_dispatch: deliver a pending signal or resume.
static void exit_to_guest(struct sigthread *st) {
  atomic_store(&st->t.state, LX_STATE_EXITING);
  lx_siginfo_t li;
  int sig = take_deliverable(st, &li);
  if (!sig) return;  // asm falls into lx_resume
  struct lx_cpu c;
  cpu_from_frame(&c, &st->t.f);
  uint64_t e[7];
  setup_rt_frame(st, sig, &li, &c, 0, e);
  atomic_store(&st->t.state, LX_STATE_GUEST);
  lx_enter_handler_c(e);
}

static struct sigthread *g_threads[64];
static _Atomic int g_nthreads;

static long sys_tgkill(int tid, int sig, struct sigthread *from) {
  for (int i = 0; i < g_nthreads; i++) {
    struct sigthread *st = g_threads[i];
    if (st && st->t.tid == tid) {
      lx_siginfo_t *li = &st->pinfo[sig];
      memset(li, 0, sizeof *li);
      li->si_signo = sig;
      li->si_code = LX_SI_TKILL;
      li->kill.pid = 4242;
      li->kill.uid = 10123;
      (void)from;
      atomic_fetch_or(&st->pending, sigbit(sig));
      pthread_kill(st->pth, SIGUSR2);
      return 0;
    }
  }
  return -ESRCH;
}

static _Noreturn void do_rt_sigreturn(struct sigthread *st) {
  // At the restorer's svc, sp points at the rt_sigframe the handler got.
  struct lx_rt_sigframe *fr = (void *)st->t.f.sp;
  struct lx_sigcontext *sc = &fr->uc.uc_mcontext;
  struct lx_cpu *c = &st->resume_cpu;
  memcpy(c->x, sc->regs, sizeof c->x);
  c->sp = sc->sp;
  c->pc = sc->pc;
  c->pstate = sc->pstate;
  for (uint8_t *p = sc->__reserved; p < sc->__reserved + sizeof sc->__reserved;) {
    struct _aarch64_ctx *h = (void *)p;
    if (h->magic == 0) break;
    if (h->magic == FPSIMD_MAGIC) {
      struct fpsimd_context *fp = (void *)p;
      memcpy(c->v, fp->vregs, sizeof c->v);
      c->fpsr = fp->fpsr;
      c->fpcr = fp->fpcr;
    }
    p += h->size;
  }
  st->resume_mask = fr->uc.uc_sigmask & ~(sigbit(9) | sigbit(19));
  atomic_store(&st->t.state, LX_STATE_RESUMING);
  __asm__ volatile("b _lx_resume_trap");
  __builtin_unreachable();
}

void lx_dispatch(struct lx_thread *t) {
  struct sigthread *st = (struct sigthread *)t;
  struct lx_frame *f = &t->f;
  long r;
  switch (f->x[8]) {
    case 134: {  // rt_sigaction(sig, act, oact, sigsetsize)
      int sig = (int)f->x[0];
      const struct lx_k_sigaction *act = (void *)f->x[1];
      struct lx_k_sigaction *oact = (void *)f->x[2];
      if (sig < 1 || sig > LX_NSIG || sig == 9 || sig == 19) { r = -EINVAL; break; }
      if (oact) *oact = g_act[sig];
      if (act) g_act[sig] = *act;
      r = 0;
      break;
    }
    case 132: {  // sigaltstack
      const lx_stack_t *ss = (void *)f->x[0];
      lx_stack_t *old = (void *)f->x[1];
      if (old) *old = st->altstack;
      if (ss) st->altstack = (ss->ss_flags & LX_SS_DISABLE) ? (lx_stack_t){0} : *ss;
      r = 0;
      break;
    }
    case 135: {  // rt_sigprocmask(how, set, oset, size)
      const uint64_t *set = (void *)f->x[1];
      uint64_t *oset = (void *)f->x[2];
      if (oset) *oset = st->guest_mask;
      if (set) {
        uint64_t s = *set & ~(sigbit(9) | sigbit(19));
        if (f->x[0] == LX_SIG_BLOCK) st->guest_mask |= s;
        else if (f->x[0] == LX_SIG_UNBLOCK) st->guest_mask &= ~s;
        else st->guest_mask = s;
      }
      r = 0;
      break;
    }
    case 139: do_rt_sigreturn(st);
    case 131: r = sys_tgkill((int)f->x[1], (int)f->x[2], st); break;  // tgkill(tgid, tid, sig)
    case 178: r = t->tid; break;
    case 98: {  // futex WAIT only, enough for the EINTR test
      uint32_t *a = (void *)f->x[0];
      if (__atomic_load_n(a, __ATOMIC_SEQ_CST) != (uint32_t)f->x[2]) { r = -EAGAIN; break; }
      r = os_sync_wait_on_address(a, (uint32_t)f->x[2], 4, 0) < 0 ? -errno : 0;
      break;
    }
    default: r = -ENOSYS;
  }
  f->x[0] = (uint64_t)r;
  exit_to_guest(st);
}

// ----------------------------------------------------------- guest side
// Everything below here stands for guest code: it only talks Linux.
long guest_syscall6(long nr, long a0, long a1, long a2, long a3, long a4, long a5);
#define GS_(nr, a, b, c, d, e, f, ...) \
  guest_syscall6((long)(nr), (long)(a), (long)(b), (long)(c), (long)(d), (long)(e), (long)(f))
#define guest_syscall(...) GS_(__VA_ARGS__, 0, 0, 0, 0, 0, 0, 0)
uint64_t guest_null_probe(uint64_t out[4]);
uint64_t guest_store_probe(void *addr);
uint64_t guest_load_probe(void *addr);
uint64_t guest_spin_probe(uint64_t out[2]);
uint64_t guest_syscall_storm(uint64_t n);
extern char guest_null_fault_pc[], guest_npe_landing[], guest_store_fault_pc[], guest_load_fault_pc[], guest_spin_loop[];

enum action { A_NPE, A_SKIP_SET_X0, A_STOP_SPIN, A_NOTHING };
static volatile enum action g_action;
static struct seen {
  int sig, code;
  uint64_t si_addr, fault_address, pc, sp, esr, x0;
  int has_fpsimd;
  uint64_t v8_lo;
  uint64_t handler_sp;
  int32_t si_pid;
  uint64_t uc_sigmask;
} g_seen;
static _Atomic int g_handler_runs;

static void guest_handler(int sig, lx_siginfo_t *si, lx_ucontext_t *uc) {
  struct lx_sigcontext *sc = &uc->uc_mcontext;
  volatile uint64_t marker;
  g_seen.sig = sig;
  g_seen.code = si->si_code;
  g_seen.si_addr = si->fault.addr;
  g_seen.si_pid = si->kill.pid;
  g_seen.fault_address = sc->fault_address;
  g_seen.pc = sc->pc;
  g_seen.sp = sc->sp;
  g_seen.x0 = sc->regs[0];
  g_seen.handler_sp = (uint64_t)&marker;
  g_seen.uc_sigmask = uc->uc_sigmask;
  g_seen.esr = 0;
  g_seen.has_fpsimd = 0;
  for (uint8_t *p = sc->__reserved;;) {  // walk the records like a Linux unwinder does
    struct _aarch64_ctx *h = (void *)p;
    if (!h->magic) break;
    if (h->magic == FPSIMD_MAGIC && h->size == 528) {
      g_seen.has_fpsimd = 1;
      g_seen.v8_lo = (uint64_t)((struct fpsimd_context *)p)->vregs[8];
    }
    if (h->magic == ESR_MAGIC) g_seen.esr = ((struct esr_context *)p)->esr;
    p += h->size;
  }
  atomic_fetch_add(&g_handler_runs, 1);
  switch (g_action) {
    case A_NPE:  // what ART does: redirect to the NPE throw path, pass state in registers
      sc->pc = (uint64_t)guest_npe_landing;
      sc->regs[1] = 0xdeadbeef;
      sc->regs[19] = 0x1234;
      for (uint8_t *p = sc->__reserved;;) {
        struct _aarch64_ctx *h = (void *)p;
        if (!h->magic) break;
        if (h->magic == FPSIMD_MAGIC) ((struct fpsimd_context *)p)->vregs[8] = 0xf00d;
        p += h->size;
      }
      break;
    case A_SKIP_SET_X0:
      sc->pc += 4;
      sc->regs[0] = 0x600d;
      break;
    case A_STOP_SPIN:
      sc->regs[10] = 1;  // the spin loop exits only if this sticks
      break;
    case A_NOTHING:
      break;
  }
}

static void guest_sigaction(int sig, uint64_t flags) {
  struct lx_k_sigaction ka = {.handler = (uint64_t)guest_handler,
                              .flags = flags | LX_SA_SIGINFO | LX_SA_RESTORER,
                              .restorer = (uint64_t)guest_restore_rt,
                              .mask = 0};
  long r = guest_syscall(134, sig, &ka, 0, 8);
  if (r) abort();
}

static int fails;
#define EXPECT(cond, ...) do { int ok_ = (cond); printf("  [%s] ", ok_ ? " ok " : "FAIL"); printf(__VA_ARGS__); printf("\n"); fails += !ok_; } while (0)

static struct sigthread *new_sigthread(int tid, uint64_t host_stack_top) {
  struct sigthread *st = calloc(1, sizeof *st);
  st->t.tid = tid;
  st->pth = pthread_self();
  lx_bind_thread(&st->t, host_stack_top);
  host_thread_signals();
  g_threads[atomic_fetch_add(&g_nthreads, 1)] = st;
  return st;
}

struct spin_arg { uint64_t out[2]; uint64_t mismatch; _Atomic int ready; int tid; };
static void *spinner(void *p) {
  struct spin_arg *a = p;
  void *hs = malloc(256 * 1024);
  new_sigthread(a->tid, (uint64_t)hs + 256 * 1024);
  atomic_store(&a->ready, 1);
  a->mismatch = guest_spin_probe(a->out);
  return NULL;
}

struct wait_arg { uint32_t word; long ret; _Atomic int ready; int tid; };
static void *futex_waiter(void *p) {
  struct wait_arg *a = p;
  void *hs = malloc(256 * 1024);
  new_sigthread(a->tid, (uint64_t)hs + 256 * 1024);
  atomic_store(&a->ready, 1);
  a->ret = guest_syscall(98, &a->word, 0 /*FUTEX_WAIT*/, 0, 0);
  return NULL;
}

struct storm_arg { uint64_t bad; _Atomic int ready, done; int tid; };
static void *stormer(void *p) {
  struct storm_arg *a = p;
  void *hs = malloc(256 * 1024);
  new_sigthread(a->tid, (uint64_t)hs + 256 * 1024);
  atomic_store(&a->ready, 1);
  a->bad = guest_syscall_storm(3000000);
  atomic_store(&a->done, 1);
  return NULL;
}

// Native Darwin baseline for the cost comparison.
static void native_handler(int s, siginfo_t *si, void *ucv) {
  (void)s; (void)si;
  ucontext_t *uc = ucv;
  uc->uc_mcontext->__ss.__pc = (uint64_t)guest_npe_landing;
}

int main(void) {
  setvbuf(stdout, NULL, _IONBF, 0);
  lx_init_process();
  install_host_handlers();
  void *hs = malloc(256 * 1024);
  new_sigthread(100, (uint64_t)hs + 256 * 1024);

  printf("Linux frame layout: sizeof(rt_sigframe)=%zu, uc_mcontext at +%zu, __reserved at +%zu of sigcontext\n",
         sizeof(struct lx_rt_sigframe), offsetof(lx_ucontext_t, uc_mcontext), offsetof(struct lx_sigcontext, __reserved));

  guest_sigaction(LX_SIGSEGV, LX_SA_ONSTACK);
  guest_sigaction(LX_SIGBUS, 0);
  guest_sigaction(LX_SIGUSR1, LX_SA_RESTART);
  guest_sigaction(40, 0);  // an RT signal: no Darwin equivalent

  // T1: implicit null check.
  printf("T1 null dereference (ART implicit null check):\n");
  g_action = A_NPE;
  uint64_t out[4] = {0};
  uint64_t landed = guest_null_probe(out);
  EXPECT(g_seen.sig == LX_SIGSEGV && g_seen.code == LX_SEGV_MAPERR, "signo=%d si_code=%d (want SIGSEGV=11, SEGV_MAPERR=1)", g_seen.sig, g_seen.code);
  EXPECT(g_seen.si_addr == 8 && g_seen.fault_address == 8, "si_addr=%#" PRIx64 " sigcontext.fault_address=%#" PRIx64 " (want 0x8)", g_seen.si_addr, g_seen.fault_address);
  EXPECT(g_seen.pc == (uint64_t)guest_null_fault_pc, "sigcontext.pc is the faulting ldr");
  EXPECT(g_seen.has_fpsimd && g_seen.v8_lo == 0xcafe, "fpsimd_context record (magic 0x46508001, size 528) carries v8=%#" PRIx64, g_seen.v8_lo);
  EXPECT((g_seen.esr >> 26) == 0x24, "esr_context present, ESR=%#" PRIx64 " (EC 0x24 = data abort from EL0)", g_seen.esr);
  EXPECT(landed == 1 && out[0] == 0xdeadbeef && out[1] == 0x1234 && out[2] == 0xf00d,
         "handler redirected pc and set x1/x19/v8: resumed at landing=%s x1=%#" PRIx64 " x19=%#" PRIx64 " v8=%#" PRIx64,
         landed == 1 ? "yes" : "no", out[0], out[1], out[2]);

  // T2: PROT_NONE guard page, handler on the guest sigaltstack.
  printf("T2 guard page (stack-overflow shape), SA_ONSTACK on a guest sigaltstack:\n");
  size_t pg = (size_t)getpagesize();
  char *guard = mmap(NULL, 2 * pg, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
  lx_stack_t alt = {.ss_sp = (uint64_t)malloc(64 * 1024), .ss_size = 64 * 1024};
  guest_syscall(132, &alt, 0);
  g_action = A_SKIP_SET_X0;
  uint64_t r = guest_store_probe(guard + 24);
  EXPECT(g_seen.sig == LX_SIGSEGV && g_seen.code == LX_SEGV_ACCERR, "signo=%d si_code=%d (want SIGSEGV, SEGV_ACCERR=2; Darwin raised SIG%s)",
         g_seen.sig, g_seen.code, g_raw_log[1].hsig == SIGBUS ? "BUS" : "SEGV");
  EXPECT(g_seen.si_addr == (uint64_t)(guard + 24), "si_addr exact (%p)", (void *)g_seen.si_addr);
  EXPECT(g_seen.handler_sp >= alt.ss_sp && g_seen.handler_sp < alt.ss_sp + alt.ss_size, "guest handler ran on the guest sigaltstack");
  EXPECT(r == 0x600d, "handler skipped the store (pc += 4) and set x0");

  // T3: write to a read-only page.
  printf("T3 write to a read-only mapping:\n");
  char *ro = mmap(NULL, pg, PROT_READ, MAP_PRIVATE | MAP_ANON, -1, 0);
  r = guest_store_probe(ro + 16);
  EXPECT(g_seen.sig == LX_SIGSEGV && g_seen.code == LX_SEGV_ACCERR && g_seen.si_addr == (uint64_t)(ro + 16) && r == 0x600d,
         "signo=%d si_code=%d addr exact=%s (Darwin raised SIG%s)", g_seen.sig, g_seen.code,
         g_seen.si_addr == (uint64_t)(ro + 16) ? "yes" : "no", g_raw_log[2].hsig == SIGBUS ? "BUS" : "SEGV");

  // T4: file mapping past EOF (Linux: SIGBUS/BUS_ADRERR).
  printf("T4 read past EOF of a mapped file:\n");
  char path[] = "/tmp/p0sigXXXXXX";
  int fd = mkstemp(path);
  unlink(path);
  (void)write(fd, "x", 1);
  char *fm = mmap(NULL, 4 * pg, PROT_READ, MAP_SHARED, fd, 0);
  r = guest_load_probe(fm + 2 * pg);
  EXPECT(g_seen.sig == LX_SIGBUS && g_seen.code == LX_BUS_ADRERR && g_seen.si_addr == (uint64_t)(fm + 2 * pg),
         "signo=%d si_code=%d addr exact=%s", g_seen.sig, g_seen.code, g_seen.si_addr == (uint64_t)(fm + 2 * pg) ? "yes" : "no");
  close(fd);

  printf("   raw Darwin view of the faults above:\n");
  for (uint64_t i = 0; i < g_raw_log_n && i < 4; i++)
    printf("     host SIG%s si_code=%d si_addr=%#" PRIx64 " FAR=%#" PRIx64 " ESR=%#" PRIx64 " (EC=%#" PRIx64 " FSC=%#" PRIx64 ")\n",
           g_raw_log[i].hsig == SIGBUS ? "BUS" : g_raw_log[i].hsig == SIGSEGV ? "SEGV" : "?", g_raw_log[i].hcode,
           g_raw_log[i].addr, g_raw_log[i].far, g_raw_log[i].esr, g_raw_log[i].esr >> 26, g_raw_log[i].esr & 0x3f);

  // T5/T6: asynchronous signals interrupting guest code at an arbitrary pc.
  const char *names[] = {"SIGUSR1 (10)", "RT signal 40"};
  int sigs[] = {LX_SIGUSR1, 40};
  for (int k = 0; k < 2; k++) {
    printf("T%d async %s via tgkill while the target spins in guest code:\n", 5 + k, names[k]);
    struct spin_arg sa = {.tid = 200 + k};
    pthread_t th;
    pthread_create(&th, NULL, spinner, &sa);
    while (!atomic_load(&sa.ready)) usleep(100);
    usleep(20000);
    g_action = A_STOP_SPIN;
    guest_syscall(131, 1, sa.tid, sigs[k]);
    pthread_join(th, NULL);
    EXPECT(g_seen.sig == sigs[k] && g_seen.code == LX_SI_TKILL && g_seen.si_pid == 4242, "signo=%d si_code=%d si_pid=%d", g_seen.sig, g_seen.code, g_seen.si_pid);
    EXPECT(g_seen.pc >= (uint64_t)guest_spin_loop && g_seen.pc < (uint64_t)guest_spin_loop + 8, "interrupted pc is inside the spin loop");
    EXPECT(sa.mismatch == 0 && sa.out[0] > 0, "handler's x10 write ended the loop after %" PRIu64 " iterations; x19/x20/x28/v8/v9/v15 intact (bitmap %#" PRIx64 ")", sa.out[0], sa.mismatch);
  }

  // T7: signal while blocked in a syscall.
  printf("T7 SIGUSR1 while the target blocks in futex(FUTEX_WAIT):\n");
  {
    struct wait_arg wa = {.tid = 300};
    pthread_t th;
    int before = g_handler_runs;
    pthread_create(&th, NULL, futex_waiter, &wa);
    while (!atomic_load(&wa.ready)) usleep(100);
    usleep(20000);
    g_action = A_SKIP_SET_X0;  // pc/x0 untouched is what matters here; use a benign action
    g_action = A_STOP_SPIN;
    guest_syscall(131, 1, wa.tid, LX_SIGUSR1);
    pthread_join(th, NULL);
    EXPECT(g_handler_runs == before + 1 && g_seen.sig == LX_SIGUSR1 && wa.ret == -EINTR,
           "handler ran at syscall exit, futex returned %ld (-EINTR=%d)", wa.ret, -EINTR);
    EXPECT(g_seen.x0 == (uint64_t)-EINTR, "sigcontext.regs[0] holds the syscall result the guest resumes with");
  }

  // T8: races. Signals land anywhere in the syscall path.
  printf("T8 signal storm against a thread looping on syscalls (races with entry/dispatch/exit):\n");
  {
    struct storm_arg sa = {.tid = 400};
    pthread_t th;
    g_action = A_NOTHING;
    int before = g_handler_runs;
    pthread_create(&th, NULL, stormer, &sa);
    while (!atomic_load(&sa.ready)) usleep(100);
    long sent = 0;
    while (!atomic_load(&sa.done)) {
      guest_syscall(131, 1, sa.tid, (sent & 1) ? LX_SIGUSR1 : 40);
      sent++;
      for (volatile int k = 0; k < 2000; k++) { }
    }
    pthread_join(th, NULL);
    int handled = g_handler_runs - before;
    EXPECT(sa.bad == 0 && handled > 0, "3M syscalls, %ld tgkills sent, %d handler runs (standard signals coalesce), iterations with corrupted registers: %" PRIu64,
           sent, handled, sa.bad);
  }

  // Cost.
  printf("cost of a guest SIGSEGV round trip (fault -> Linux frame -> guest handler -> rt_sigreturn -> resume):\n");
  enum { N = 20000 };
  g_action = A_NPE;
  uint64_t t0 = now_ns();
  for (int i = 0; i < N; i++) guest_null_probe(out);
  uint64_t dt = now_ns() - t0;
  printf("  lx path:         %.2f us per fault\n", dt / 1e3 / N);
  struct sigaction nsa = {0}, old;
  nsa.sa_sigaction = native_handler;
  nsa.sa_flags = SA_SIGINFO | SA_ONSTACK;
  sigaction(SIGSEGV, &nsa, &old);
  t0 = now_ns();
  for (int i = 0; i < N; i++) guest_null_probe(out);
  dt = now_ns() - t0;
  sigaction(SIGSEGV, &old, NULL);
  printf("  native Darwin:   %.2f us per fault (one host signal, pc fixed in the Darwin mcontext)\n", dt / 1e3 / N);
  printf("%s (%d failures)\n", fails ? "FAIL" : "PASS", fails);
  return fails != 0;
}
