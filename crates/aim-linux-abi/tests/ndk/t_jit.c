// Application JITs: V8's code range pattern (a large PROT_NONE
// reservation, aligned by over-reserving and trimming, made RWX with
// mprotect, then written and run), RWX mmaps, and threads and fork
// children running JIT code.
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

#define PG 16384
#define GB (1ull << 30)
#define MB (1ull << 20)

typedef int (*fn_t)(void);

// Write "mov w0, #v; ret" at p and run it.
static int emit_and_run(void* p, int v) {
  uint32_t* c = p;
  c[0] = 0x52800000u | ((uint32_t)v << 5);
  c[1] = 0xd65f03c0u;
  __builtin___clear_cache((char*)c, (char*)(c + 2));
  return ((fn_t)c)();
}

static sigjmp_buf jb;

static void on_segv(int sig) {
  siglongjmp(jb, 1);
}

// Whether a write to p succeeds.
static int writable(volatile char* p) {
  struct sigaction sa = {.sa_handler = on_segv}, old;
  sigaction(SIGSEGV, &sa, &old);
  int ok = 0;
  if (!sigsetjmp(jb, 1)) {
    p[8] = p[8];
    ok = 1;
  }
  sigaction(SIGSEGV, &old, NULL);
  return ok;
}

// An aligned PROT_NONE reservation, as v8::base::OS::Allocate makes one.
static char* reserve_aligned(size_t size, size_t align) {
  size_t req = size + align - PG;
  char* p = mmap((void*)0x2a0000000000ull, req, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
  if (p == MAP_FAILED) return p;
  char* a = (char*)(((uintptr_t)p + align - 1) & ~(uintptr_t)(align - 1));
  if (a != p) munmap(p, a - p);
  if (p + req != a + size) munmap(a + size, p + req - (a + size));
  return a;
}

static void v8_code_range(void) {
  // The pointer compression cage and the code range in it.
  char* cage = reserve_aligned(4 * GB, 4 * GB);
  CHECK(cage != MAP_FAILED && ((uintptr_t)cage & (4 * GB - 1)) == 0);
  char* code = cage + 256 * MB;
  size_t len = 128 * MB;
  // Pre-commit RWX, then discard.
  CHECK(mprotect(code, len, PROT_READ | PROT_WRITE | PROT_EXEC) == 0);
  CHECK(madvise(code, len, MADV_DONTNEED) == 0);
  CHECK(emit_and_run(code, 42) == 42);
  CHECK(emit_and_run(code, 43) == 43);  // rewritten in place
  CHECK(emit_and_run(code + len - PG, 7) == 7);
  // Data and code in the same pages.
  uint32_t* d = (uint32_t*)(code + 64);
  *d = 0x1234;
  CHECK(emit_and_run(code, 1) == 1 && *d == 0x1234);
  // Other protections apply, and the contents stay.
  CHECK(mprotect(code, PG, PROT_READ | PROT_EXEC) == 0);
  CHECK(((fn_t)code)() == 1 && *d == 0x1234);
  CHECK(!writable(code));
  CHECK(mprotect(code, PG, PROT_READ | PROT_WRITE | PROT_EXEC) == 0);
  CHECK(((fn_t)code)() == 1 && emit_and_run(code, 2) == 2);
  CHECK(mprotect(code, 512 * MB, PROT_NONE) == 0);  // JIT pages and not
  CHECK(mprotect(code, PG, PROT_READ | PROT_WRITE) == 0 && *d == 0x1234);
  CHECK(mprotect(code, PG, PROT_READ | PROT_WRITE | PROT_EXEC) == 0 && ((fn_t)code)() == 2);
  // Decommit (mmap over it) and commit again.
  CHECK(mmap(code + PG, PG, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED | MAP_NORESERVE, -1, 0) == code + PG);
  CHECK(mprotect(code + PG, PG, PROT_READ | PROT_WRITE | PROT_EXEC) == 0);
  CHECK(emit_and_run(code + PG, 9) == 9);
  CHECK(((fn_t)code)() == 2);
  // Contents written before the pages become RWX stay.
  char* rw = code + len + PG;
  CHECK(mprotect(rw, PG, PROT_READ | PROT_WRITE) == 0);
  rw[100] = 'k';
  CHECK(mprotect(rw, PG, PROT_READ | PROT_WRITE | PROT_EXEC) == 0 && rw[100] == 'k');
  CHECK(emit_and_run(rw, 11) == 11 && rw[100] == 'k');
  // Part of the reservation was never mapped RWX-able: a hole fails.
  CHECK(munmap(code + len - 2 * PG, PG) == 0);
  CHECK(mprotect(code + len - 3 * PG, 3 * PG, PROT_READ | PROT_WRITE | PROT_EXEC) == -1 && errno == ENOMEM);
  CHECK(munmap(cage, 4 * GB) == 0);
}

static void rwx_mmap(void) {
  char* p = mmap(NULL, 4 * PG, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(p != MAP_FAILED && p[0] == 0);
  CHECK(emit_and_run(p + PG, 21) == 21);
  // MAP_FIXED over a mapping of ours.
  char* q = mmap(p, PG, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
  CHECK(q == p && emit_and_run(q, 22) == 22);
  CHECK(((fn_t)(p + PG))() == 21);
  CHECK(munmap(p, 4 * PG) == 0);
}

static char* shared_code;
static volatile int stop;

static void* runner(void* arg) {
  long n = 0;
  while (!stop) {
    int v = ((fn_t)shared_code)();
    if (v != 30 && v != 31) return (void*)-1;
    n++;
  }
  return (void*)n;
}

static void threads(void) {
  shared_code = mmap(NULL, 2 * PG, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(shared_code != MAP_FAILED);
  CHECK(emit_and_run(shared_code, 30) == 30);
  pthread_t t;
  CHECK(pthread_create(&t, NULL, runner, NULL) == 0);
  // This thread writes and runs other code while the other one runs.
  for (int i = 0; i < 2000; i++) {
    CHECK(emit_and_run(shared_code + PG, i & 0xff) == (i & 0xff));
  }
  stop = 1;
  void* r;
  CHECK(pthread_join(t, &r) == 0 && (long)r > 0);
  munmap(shared_code, 2 * PG);
}

static void fork_child(void) {
  char* p = mmap(NULL, PG, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(p != MAP_FAILED && emit_and_run(p, 50) == 50);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    int ok = ((fn_t)p)() == 50 && emit_and_run(p, 51) == 51;
    _exit(ok ? 0 : 1);
  }
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st) && WEXITSTATUS(st) == 0);
  CHECK(((fn_t)p)() == 50);  // the child's code was its own
  munmap(p, PG);
}

int main(void) {
  RUN(v8_code_range);
  RUN(rwx_mmap);
  RUN(threads);
  RUN(fork_child);
  DONE();
}
