// P0 experiment 5: can we give ART its dual-mapped JIT code cache on macOS?
//
// ART (JitMemoryRegion) creates the cache with memfd_create and maps it
// twice: a writable view it emits code into and an executable view it
// runs. Each candidate mechanism below runs in a forked child (a
// code-signing kill must not take the whole run down) and reports:
//   map   - could both views be created
//   exec  - code written via the writable view runs via the executable view
//   rewrite - rewriting through the writable view + sys_icache_invalidate on
//           the executable view makes the new code visible (1000 rounds)
// The binary is re-signed several ways by run.sh; the matrix is the result.
#include <errno.h>
#include <fcntl.h>
#include <libkern/OSCacheControl.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define SZ (64 * 1024)
typedef int (*fn_t)(void);
static inline uint64_t now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }

// mov w0, #imm ; ret
static void emit(uint32_t *w, int imm) {
  w[0] = 0x52800000u | ((uint32_t)(imm & 0xffff) << 5);
  w[1] = 0xd65f03c0u;
}

struct views { void *rw, *rx; const char *err; };

static int check(struct views v, int toggle_jit) {
  // exec
  if (toggle_jit) pthread_jit_write_protect_np(0);
  emit(v.rw, 41);
  if (toggle_jit) pthread_jit_write_protect_np(1);
  sys_icache_invalidate(v.rx, 8);
  if (((fn_t)v.rx)() != 41) return 2;
  // rewrite in place, repeatedly (JIT code cache reuse)
  int stale = 0;
  for (int i = 0; i < 1000; i++) {
    if (toggle_jit) pthread_jit_write_protect_np(0);
    emit(v.rw, i);
    if (toggle_jit) pthread_jit_write_protect_np(1);
    sys_icache_invalidate(v.rx, 8);
    if (((fn_t)v.rx)() != i) stale++;
  }
  return stale ? 3 : 0;
}

// ---- mechanisms
static struct views m_mprotect_single(void) {
  void *p = mmap(NULL, SZ, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
  emit(p, 0);
  if (mprotect(p, SZ, PROT_READ | PROT_EXEC)) return (struct views){.err = strerror(errno)};
  // single view: "rw" must be re-opened per write; the check writes via mprotect toggles below
  return (struct views){p, p};
}
static struct views m_rwx_anon(void) {
  void *p = mmap(NULL, SZ, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANON, -1, 0);
  if (p == MAP_FAILED) return (struct views){.err = strerror(errno)};
  return (struct views){p, p};
}
static struct views m_map_jit(void) {
  void *p = mmap(NULL, SZ, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANON | MAP_JIT, -1, 0);
  if (p == MAP_FAILED) return (struct views){.err = strerror(errno)};
  return (struct views){p, p};
}
static const char *kr_str(kern_return_t kr) { static char b[64]; snprintf(b, sizeof b, "%s (%d)", mach_error_string(kr), kr); return b; }
static struct views remap_rx(mach_vm_address_t rw) {
  mach_vm_address_t rx = 0;
  vm_prot_t cur, max;
  kern_return_t kr = mach_vm_remap(mach_task_self(), &rx, SZ, 0, VM_FLAGS_ANYWHERE, mach_task_self(), rw, FALSE, &cur, &max, VM_INHERIT_NONE);
  if (kr) return (struct views){.err = kr_str(kr)};
  kr = mach_vm_protect(mach_task_self(), rx, SZ, FALSE, VM_PROT_READ | VM_PROT_EXECUTE);
  if (kr) return (struct views){.err = kr_str(kr)};
  return (struct views){(void *)rw, (void *)rx};
}
static struct views m_vm_remap(void) {
  mach_vm_address_t rw = 0;
  kern_return_t kr = mach_vm_allocate(mach_task_self(), &rw, SZ, VM_FLAGS_ANYWHERE);
  if (kr) return (struct views){.err = kr_str(kr)};
  return remap_rx(rw);
}
static struct views m_map_jit_remap_rw(void) {
  // MAP_JIT region as the RX view, plus a remapped alias kept writable.
  void *p = mmap(NULL, SZ, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANON | MAP_JIT, -1, 0);
  if (p == MAP_FAILED) return (struct views){.err = strerror(errno)};
  mach_vm_address_t rw = 0;
  vm_prot_t cur, max;
  kern_return_t kr = mach_vm_remap(mach_task_self(), &rw, SZ, 0, VM_FLAGS_ANYWHERE, mach_task_self(), (mach_vm_address_t)p, FALSE, &cur, &max, VM_INHERIT_NONE);
  if (kr) return (struct views){.err = kr_str(kr)};
  kr = mach_vm_protect(mach_task_self(), rw, SZ, FALSE, VM_PROT_READ | VM_PROT_WRITE);
  if (kr) return (struct views){.err = kr_str(kr)};
  return (struct views){(void *)rw, p};
}
static struct views mem_entry(vm_prot_t entry_prot) {
  memory_object_size_t size = SZ;
  mach_port_t entry = MACH_PORT_NULL;
  kern_return_t kr = mach_make_memory_entry_64(mach_task_self(), &size, 0, MAP_MEM_NAMED_CREATE | entry_prot, &entry, MACH_PORT_NULL);
  if (kr) return (struct views){.err = kr_str(kr)};
  mach_vm_address_t rw = 0, rx = 0;
  kr = mach_vm_map(mach_task_self(), &rw, SZ, 0, VM_FLAGS_ANYWHERE, entry, 0, FALSE, VM_PROT_READ | VM_PROT_WRITE, VM_PROT_READ | VM_PROT_WRITE, VM_INHERIT_SHARE);
  if (kr) return (struct views){.err = kr_str(kr)};
  kr = mach_vm_map(mach_task_self(), &rx, SZ, 0, VM_FLAGS_ANYWHERE, entry, 0, FALSE, VM_PROT_READ | VM_PROT_EXECUTE, VM_PROT_READ | VM_PROT_EXECUTE, VM_INHERIT_SHARE);
  if (kr) return (struct views){.err = kr_str(kr)};
  return (struct views){(void *)rw, (void *)rx};
}
static struct views m_mem_entry_rw(void) { return mem_entry(VM_PROT_READ | VM_PROT_WRITE); }
static struct views m_mem_entry_rwx(void) { return mem_entry(VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE); }
static struct views fd_views(int fd) {
  ftruncate(fd, SZ);
  void *rw = mmap(NULL, SZ, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  if (rw == MAP_FAILED) return (struct views){.err = strerror(errno)};
  void *rx = mmap(NULL, SZ, PROT_READ | PROT_EXEC, MAP_SHARED, fd, 0);
  if (rx == MAP_FAILED) return (struct views){.err = strerror(errno)};
  return (struct views){rw, rx};
}
static struct views m_shm(void) {
  char name[64];
  snprintf(name, sizeof name, "/p0jit.%d", getpid());
  int fd = shm_open(name, O_RDWR | O_CREAT | O_EXCL, 0600);
  shm_unlink(name);
  if (fd < 0) return (struct views){.err = strerror(errno)};
  return fd_views(fd);
}
static struct views m_file(void) {
  char path[] = "/tmp/p0jitXXXXXX";
  int fd = mkstemp(path);
  unlink(path);
  if (fd < 0) return (struct views){.err = strerror(errno)};
  return fd_views(fd);
}

struct mech { const char *name; struct views (*make)(void); int kind; };  // kind 0 dual, 1 mprotect-toggle, 2 jit-toggle, 3 rwx
static const struct mech mechs[] = {
  {"mprotect RW->RX, one view", m_mprotect_single, 1},
  {"anon RWX, no MAP_JIT", m_rwx_anon, 3},
  {"MAP_JIT + pthread_jit_write_protect_np", m_map_jit, 2},
  {"mach_vm_remap RW -> RX alias", m_vm_remap, 0},
  {"MAP_JIT RX + mach_vm_remap RW alias", m_map_jit_remap_rw, 0},
  {"memory entry (R|W) mapped RW + RX", m_mem_entry_rw, 0},
  {"memory entry (R|W|X) mapped RW + RX", m_mem_entry_rwx, 0},
  {"shm_open fd mmap RW + RX (memfd-like)", m_shm, 0},
  {"unlinked file fd mmap RW + RX", m_file, 0},
};

static int run_one(const struct mech *m) {
  struct views v = m->make();
  if (v.err) { printf("MAP-FAIL %s", v.err); return 1; }
  int r;
  if (m->kind == 1) {
    // single view: toggle protection around every write
    int stale = 0;
    for (int i = 0; i < 1000; i++) {
      if (mprotect(v.rw, SZ, PROT_READ | PROT_WRITE)) { printf("mprotect RW: %s", strerror(errno)); return 1; }
      emit(v.rw, i);
      if (mprotect(v.rx, SZ, PROT_READ | PROT_EXEC)) { printf("mprotect RX: %s", strerror(errno)); return 1; }
      sys_icache_invalidate(v.rx, 8);
      if (((fn_t)v.rx)() != i) stale++;
    }
    r = stale ? 3 : 0;
  } else {
    r = check(v, m->kind == 2);
  }
  printf("%s", r == 0 ? "OK" : r == 2 ? "WRONG-RESULT" : "STALE");
  return r;
}

// What Linux guest code (ART's FlushInstructionCache, libgcc's
// __clear_cache) does itself: read CTR_EL0, then dc cvau / ic ivau by line.
static void guest_style_clear_cache(char *b, char *e) {
  uint64_t ctr;
  __asm__ volatile("mrs %0, ctr_el0" : "=r"(ctr));
  uint64_t dline = 4u << ((ctr >> 16) & 0xf), iline = 4u << (ctr & 0xf);
  for (uintptr_t p = (uintptr_t)b & ~(dline - 1); p < (uintptr_t)e; p += dline) __asm__ volatile("dc cvau, %0" ::"r"(p) : "memory");
  __asm__ volatile("dsb ish" ::: "memory");
  for (uintptr_t p = (uintptr_t)b & ~(iline - 1); p < (uintptr_t)e; p += iline) __asm__ volatile("ic ivau, %0" ::"r"(p) : "memory");
  __asm__ volatile("dsb ish\n isb" ::: "memory");
}

static void guest_cache_ops(void) {
  pid_t pid = fork();
  if (pid == 0) {
    uint64_t ctr;
    __asm__ volatile("mrs %0, ctr_el0" : "=r"(ctr));
    printf("  EL0 cache maintenance as Linux guests do it: CTR_EL0=%#llx (IDC=%llu DIC=%llu)",
           (unsigned long long)ctr, (unsigned long long)(ctr >> 28) & 1, (unsigned long long)(ctr >> 29) & 1);
    struct views v = m_mem_entry_rwx();
    if (v.err) _exit(1);
    int bad = 0;
    for (int i = 0; i < 10000; i++) {
      emit(v.rw, i);
      guest_style_clear_cache(v.rw, (char *)v.rw + 8);
      guest_style_clear_cache(v.rx, (char *)v.rx + 8);
      if (((fn_t)v.rx)() != i) bad++;
    }
    printf(", dc cvau/ic ivau executed, %d stale of 10000\n", bad);
    _exit(0);
  }
  int st;
  waitpid(pid, &st, 0);
  if (WIFSIGNALED(st)) printf("  guest-style __clear_cache: KILLED by signal %d (its mrs ctr_el0 traps; see el0_insns)\n", WTERMSIG(st));
}

static void costs(void) {
  guest_cache_ops();
  printf("\ncosts (only meaningful where the mechanism works):\n");
  void *j = mmap(NULL, SZ, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANON | MAP_JIT, -1, 0);
  if (j != MAP_FAILED) {
    enum { N = 1000000 };
    uint64_t t0 = now_ns();
    for (int i = 0; i < N; i++) { pthread_jit_write_protect_np(0); pthread_jit_write_protect_np(1); }
    printf("  pthread_jit_write_protect_np(0)+(1): %.1f ns per pair\n", (now_ns() - t0) / (double)N);
  }
  void *p = mmap(NULL, SZ, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
  memset(p, 0, SZ);
  enum { M = 100000 };
  uint64_t t0 = now_ns();
  int ok = 1;
  for (int i = 0; i < M && ok; i++) ok = !mprotect(p, 16384, PROT_READ | PROT_EXEC) && !mprotect(p, 16384, PROT_READ | PROT_WRITE);
  if (ok) printf("  mprotect RX+RW on one 16 KiB page: %.1f ns per pair\n", (now_ns() - t0) / (double)M);
  // Is sys_icache_invalidate actually needed? Rewrite through the RW view of
  // a dual mapping and call through the RX view with and without it.
  struct views v = m_mem_entry_rwx();
  if (!v.err) {
    for (int inval = 0; inval < 2; inval++) {
      int stale = 0;
      for (int i = 0; i < 100000; i++) {
        emit(v.rw, i & 0xffff);
        if (inval) sys_icache_invalidate(v.rx, 8);
        if (((fn_t)v.rx)() != (i & 0xffff)) stale++;
      }
      printf("  dual view, 100000 rewrites %s sys_icache_invalidate: %d stale executions\n", inval ? "with" : "without", stale);
    }
  }
  size_t sizes[] = {64, 4096, 65536};
  for (int k = 0; k < 3; k++) {
    t0 = now_ns();
    for (int i = 0; i < M; i++) sys_icache_invalidate(p, sizes[k]);
    printf("  sys_icache_invalidate(%zu bytes): %.1f ns\n", sizes[k], (now_ns() - t0) / (double)M);
  }
}

int main(int argc, char **argv) {
  setvbuf(stdout, NULL, _IONBF, 0);
  printf("signing: %s\n", argc > 1 ? argv[1] : "(unspecified)");
  for (unsigned i = 0; i < sizeof mechs / sizeof mechs[0]; i++) {
    printf("  %-42s ", mechs[i].name);
    pid_t pid = fork();
    if (pid == 0) _exit(run_one(&mechs[i]));
    int st;
    waitpid(pid, &st, 0);
    if (WIFSIGNALED(st)) printf("KILLED by signal %d%s", WTERMSIG(st), WTERMSIG(st) == SIGKILL ? " (code signing)" : "");
    printf("\n");
  }
  if (argc > 2) costs();
  return 0;
}
