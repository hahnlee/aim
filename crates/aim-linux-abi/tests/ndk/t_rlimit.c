// RLIMIT_AS and RLIMIT_DATA count the process's mappings (may_expand_vm):
// a mapping past the limit fails with ENOMEM, one below it succeeds, and a
// forked child keeps both the limit and what is mapped. Each case runs in a
// child, so the limits stay there.
#include <stdint.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

#define MIB (1024UL * 1024)
#define CHUNK (64 * MIB)
#define MAX_CHUNKS 64

static int set_limit(int resource, rlim_t soft) {
  struct rlimit l = {soft, RLIM_INFINITY};
  return setrlimit(resource, &l);
}

static void* map(size_t len, int prot, int flags) {
  return mmap(NULL, len, prot, MAP_ANONYMOUS | flags, -1, 0);
}

// What the process has mapped, as the limit `resource` counts it: the
// lowest limit under which one more page of `prot` (private) maps. Found
// by halving, so it needs no knowledge of what bionic reserved (scudo's
// regions alone are gigabytes).
static size_t in_use(int resource, int prot) {
  size_t lo = 0, hi = (size_t)1 << 47;
  while (hi - lo > 16384) {
    size_t mid = lo + (hi - lo) / 2;
    if (set_limit(resource, mid) != 0) return 0;
    void* p = map(16384, prot, MAP_PRIVATE);
    if (p == MAP_FAILED) {
      lo = mid;
    } else {
      munmap(p, 16384);
      hi = mid;
    }
  }
  set_limit(resource, RLIM_INFINITY);
  return hi - 16384;
}

// Maps CHUNK-sized mappings until one fails; their number, or -1 if the
// failure was not ENOMEM.
static int fill(void** chunks, int prot, int flags) {
  for (int i = 0; i < MAX_CHUNKS; i++) {
    chunks[i] = map(CHUNK, prot, flags);
    if (chunks[i] == MAP_FAILED) return errno == ENOMEM ? i : -1;
  }
  return MAX_CHUNKS;
}

// The child's checks of RLIMIT_AS; its exit status names the one that
// failed.
static int address_space_child(void) {
  size_t used = in_use(RLIMIT_AS, PROT_NONE);
  if (used == 0) return 2;
  // Room for exactly 8 chunks.
  rlim_t limit = used + 8 * CHUNK;
  if (set_limit(RLIMIT_AS, limit) != 0) return 2;
  struct rlimit got;
  if (getrlimit(RLIMIT_AS, &got) != 0 || got.rlim_cur != limit) return 3;
  // Reservations count too (MAP_NORESERVE, PROT_NONE).
  if (map(9 * CHUNK, PROT_NONE, MAP_PRIVATE | MAP_NORESERVE) != MAP_FAILED || errno != ENOMEM)
    return 4;
  void* chunks[MAX_CHUNKS];
  if (fill(chunks, PROT_NONE, MAP_PRIVATE | MAP_NORESERVE) != 8) return 5;
  // A fixed mapping over pages already mapped needs no room.
  if (mmap(chunks[0], CHUNK, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0) !=
      chunks[0])
    return 6;
  // Growing a mapping past the limit fails.
  if (mremap(chunks[1], CHUNK, 2 * CHUNK, MREMAP_MAYMOVE) != MAP_FAILED || errno != ENOMEM)
    return 7;
  // The child of a fork has the same limit and the same mappings.
  pid_t pid = fork();
  if (pid < 0) return 8;
  if (pid == 0) {
    if (map(CHUNK, PROT_NONE, MAP_PRIVATE) != MAP_FAILED || errno != ENOMEM) _exit(9);
    if (munmap(chunks[2], CHUNK) != 0) _exit(10);
    if (map(CHUNK, PROT_NONE, MAP_PRIVATE) == MAP_FAILED) _exit(11);
    _exit(0);
  }
  int status = 0;
  if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) return 12;
  if (WEXITSTATUS(status) != 0) return WEXITSTATUS(status);
  // Unmapping makes room again.
  if (munmap(chunks[3], CHUNK) != 0) return 13;
  if (map(CHUNK, PROT_NONE, MAP_PRIVATE) == MAP_FAILED) return 14;
  return 0;
}

// The child's checks of RLIMIT_DATA, which counts private writable
// mappings and the program break.
static int data_child(void) {
  size_t used = in_use(RLIMIT_DATA, PROT_READ | PROT_WRITE);
  if (used == 0 || set_limit(RLIMIT_DATA, used + 4 * CHUNK) != 0) return 2;
  void* chunks[MAX_CHUNKS];
  if (fill(chunks, PROT_READ | PROT_WRITE, MAP_PRIVATE) != 4) return 3;
  // Not data: read-only, inaccessible and shared mappings.
  void* ro = map(CHUNK, PROT_READ, MAP_PRIVATE);
  if (ro == MAP_FAILED) return 4;
  if (map(CHUNK, PROT_READ | PROT_WRITE, MAP_SHARED) == MAP_FAILED) return 5;
  // A private mapping becoming writable becomes data.
  if (mprotect(ro, CHUNK, PROT_READ | PROT_WRITE) != -1 || errno != ENOMEM) return 6;
  // So is the heap: the break does not move past the limit.
  void* before = sbrk(0);
  if (sbrk(CHUNK) != (void*)-1 || errno != ENOMEM || sbrk(0) != before) return 7;
  if (munmap(chunks[0], CHUNK) != 0) return 8;
  if (mprotect(ro, CHUNK, PROT_READ | PROT_WRITE) != 0) return 9;
  return 0;
}

static void in_child(int (*f)(void)) {
  FORK_OR_SKIP(pid);
  if (pid == 0) _exit(f());
  int status = 0;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status));
  errno = 0;
  if (WEXITSTATUS(status) != 0) printf("child check %d failed\n", WEXITSTATUS(status));
  CHECK(WEXITSTATUS(status) == 0);
}

static void address_space_limit(void) { in_child(address_space_child); }

static void data_limit(void) { in_child(data_child); }

int main(void) {
  RUN(address_space_limit);
  RUN(data_limit);
  DONE();
}
