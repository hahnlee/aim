// Memory syscalls: mremap, madvise, mincore, msync, mlock, membarrier,
// process_vm_readv, private file mappings, and ART's dual-mapped JIT cache
// over a memfd.
#include <fcntl.h>
#include <linux/membarrier.h>
#include <setjmp.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

#define PG 16384

static char* anon(size_t len, int prot, int flags) {
  return mmap(NULL, len, prot, MAP_ANONYMOUS | flags, -1, 0);
}

static void mremap_moves(void) {
  char* a = anon(2 * PG, PROT_READ | PROT_WRITE, MAP_PRIVATE);
  CHECK(a != MAP_FAILED);
  memset(a, 'a', 2 * PG);
  // Shrink in place.
  CHECK(mremap(a, 2 * PG, PG, 0) == a && a[PG - 1] == 'a');
  // Grow, moving if needed; the contents follow.
  char* b = mremap(a, PG, 8 * PG, MREMAP_MAYMOVE);
  CHECK(b != MAP_FAILED && b[0] == 'a' && b[PG - 1] == 'a' && b[7 * PG] == 0);
  b[8 * PG - 1] = 'z';
  // Move to a fixed place (bionic's CFI shadow does this).
  char* dst = anon(8 * PG, PROT_NONE, MAP_PRIVATE);
  CHECK(mprotect(b, 8 * PG, PROT_READ) == 0);
  char* c = mremap(b, 8 * PG, 8 * PG, MREMAP_MAYMOVE | MREMAP_FIXED, dst);
  CHECK(c == dst && c[0] == 'a' && c[8 * PG - 1] == 'z');
  unsigned char v;
  CHECK(mincore(b, PG, &v) == -1 && errno == ENOMEM);  // old range is gone
  CHECK(mremap(c, PG, PG, MREMAP_FIXED, c + PG) == MAP_FAILED && errno == EINVAL);
  munmap(c, 8 * PG);
  // old_size 0 duplicates a shared mapping.
  char* s = anon(PG, PROT_READ | PROT_WRITE, MAP_SHARED);
  char* s2 = mremap(s, 0, PG, MREMAP_MAYMOVE);
  CHECK(s2 != MAP_FAILED && s2 != s);
  s[5] = 'q';
  CHECK(s2[5] == 'q');
  munmap(s, PG);
  munmap(s2, PG);
  // Growing in place without MAYMOVE fails when the next page is taken.
  char* g = anon(2 * PG, PROT_READ | PROT_WRITE, MAP_PRIVATE);
  CHECK(munmap(g + PG, PG) == 0);
  char* blocker = mmap(g + PG, PG, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
  CHECK(blocker == g + PG);
  CHECK(mremap(g, PG, 2 * PG, 0) == MAP_FAILED && errno == ENOMEM);
  munmap(g, 2 * PG);
}

static void madvise_dontneed(void) {
  char* a = anon(4 * PG, PROT_READ | PROT_WRITE, MAP_PRIVATE);
  memset(a, 7, 4 * PG);
  CHECK(madvise(a + PG, 2 * PG, MADV_DONTNEED) == 0);
  CHECK(a[0] == 7 && a[PG] == 0 && a[3 * PG - 1] == 0 && a[3 * PG] == 7);
  a[PG] = 1;  // still writable
  munmap(a, 4 * PG);
  char* s = anon(PG, PROT_READ | PROT_WRITE, MAP_SHARED);
  s[0] = 9;
  CHECK(madvise(s, PG, MADV_DONTNEED) == 0 && s[0] == 9);
  munmap(s, PG);
  // A private file mapping reads the file again.
  int fd = open("/system/build.prop", O_RDONLY);
  char* f = mmap(NULL, PG, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
  CHECK(f != MAP_FAILED);
  char first = f[0];
  f[0] = first + 1;
  CHECK(madvise(f, PG, MADV_DONTNEED) == 0 && f[0] == first);
  munmap(f, PG);
  close(fd);
  CHECK(madvise(a + 1, PG, MADV_DONTNEED) == -1 && errno == EINVAL);
}

static void mincore_msync_mlock(void) {
  char* a = anon(2 * PG, PROT_READ | PROT_WRITE, MAP_PRIVATE);
  a[0] = 1;
  unsigned char v[2] = {0xff, 0xff};
  CHECK(mincore(a, 2 * PG, v) == 0 && (v[0] & 1) && (v[0] & ~1) == 0 && (v[1] & ~1) == 0);
  CHECK(mlock(a, PG) == 0 && munlock(a, PG) == 0);
  CHECK(msync(a, PG, MS_SYNC) == 0);
  CHECK(msync(a, PG, MS_SYNC | MS_ASYNC) == -1 && errno == EINVAL);
  munmap(a, 2 * PG);
}

static void membarrier_and_vm_readv(void) {
  long q = syscall(SYS_membarrier, MEMBARRIER_CMD_QUERY, 0, 0);
  CHECK(q > 0 && (q & MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE));
  CHECK(syscall(SYS_membarrier, MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE, 0, 0) == 0);
  CHECK(syscall(SYS_membarrier, MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE, 0, 0) == 0);
  char src[16] = "process_vm_read";
  char dst[16] = {0};
  struct iovec l[2] = {{dst, 7}, {dst + 7, 9}};
  struct iovec r = {src, 16};
  CHECK(process_vm_readv(getpid(), l, 2, &r, 1, 0) == 16 && !memcmp(dst, src, 16));
  struct iovec bad = {(void*)16, 16};
  CHECK(process_vm_readv(getpid(), l, 1, &bad, 1, 0) == -1 && errno == EFAULT);
}

typedef int (*fn_t)(void);

// Whether /proc/self/maps names `path` for the mapping at `at`.
static int maps_names(void* at, const char* path) {
  static char buf[1 << 20];
  int fd = open("/proc/self/maps", O_RDONLY);
  size_t n = 0;
  ssize_t r;
  while (fd >= 0 && n < sizeof buf - 1 && (r = read(fd, buf + n, sizeof buf - 1 - n)) > 0) n += r;
  close(fd);
  buf[n] = 0;
  char start[32];
  snprintf(start, sizeof start, "\n%lx-", (unsigned long)at);
  char* line = strstr(buf, start);
  char* end = line ? strchr(line + 1, '\n') : NULL;
  if (!line || !end) return 0;
  *end = 0;
  return strstr(line, path) != NULL;
}

static sigjmp_buf bus_jmp;

static void on_bus(int sig) {
  siglongjmp(bus_jmp, sig);
}

// MAP_PRIVATE of a file: copy-on-write of the file's pages, as on Linux.
static void private_file_mappings(void) {
  const char* path = "/data/local/tmp/t_mem_private";
  int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
  CHECK(fd >= 0);
  static char page[PG];
  for (int i = 0; i < 3; i++) {
    memset(page, 'f' + i, PG);
    CHECK(pwrite(fd, page, PG, (off_t)i * PG) == PG);
  }
  char* p = mmap(NULL, 2 * PG, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, PG);
  char* q = mmap(NULL, 2 * PG, PROT_READ, MAP_PRIVATE, fd, PG);
  char* s = mmap(NULL, 2 * PG, PROT_READ, MAP_SHARED, fd, PG);
  CHECK(p != MAP_FAILED && q != MAP_FAILED && s != MAP_FAILED);
  CHECK(p[0] == 'g' && p[2 * PG - 1] == 'h' && q[PG] == 'h');
  // A write stays in its mapping: not in the file, nor in other mappings.
  p[0] = 'w';
  char b = 0;
  CHECK(pread(fd, &b, 1, PG) == 1 && b == 'g' && q[0] == 'g' && s[0] == 'g');
  // Whether a later change to the file shows is unspecified (mmap(2)):
  // Linux shows it in pages not written yet; Darwin, as the copies before
  // it, keeps the file as it was at mmap.
  CHECK(pwrite(fd, "xy", 2, PG) == 2);
  CHECK(q[0] == 'g' && p[0] == 'w' && p[1] == 'g' && s[0] == 'x');
  CHECK(maps_names(q, path));
  // A page wholly past the end of the file faults.
  char* past = mmap(NULL, 4 * PG, PROT_READ, MAP_PRIVATE, fd, 0);
  CHECK(past != MAP_FAILED);
  struct sigaction sa = {.sa_handler = on_bus}, old;
  sigaction(SIGBUS, &sa, &old);
  volatile char v = 0;
  int sig = sigsetjmp(bus_jmp, 1);
  if (sig == 0) v = past[3 * PG];
  sigaction(SIGBUS, &old, NULL);
  munmap(past, 4 * PG);
  CHECK(sig == SIGBUS && v == 0);
  // A fork child has the same view; MADV_DONTNEED reads the file again.
  FORK_OR_SKIP(c);
  if (c == 0) {
    int step = p[0] != 'w'                          ? 1
               : q[0] != 'g'                        ? 2
               : madvise(p, PG, MADV_DONTNEED) != 0 ? 3
               : p[0] != 'x'                        ? 4
               : !maps_names(q, path)               ? 5
                                                    : 0;
    _exit(step);
  }
  int st;
  CHECK(waitpid(c, &st, 0) == c && WIFEXITED(st));
  int step = WEXITSTATUS(st);
  if (step) printf("child failed at step %d\n", step);
  CHECK(step == 0 && p[0] == 'w');
  munmap(p, 2 * PG);
  munmap(q, 2 * PG);
  munmap(s, 2 * PG);
  // Code in a private file mapping is rewritten when it becomes
  // executable; the file keeps the original.
  uint32_t code[3] = {0xd2801588 /* mov x8, #172 (getpid) */, 0xd4000001 /* svc #0 */,
                      0xd65f03c0 /* ret */};
  CHECK(pwrite(fd, code, sizeof code, 0) == sizeof code);
  char* x = mmap(NULL, PG, PROT_READ, MAP_PRIVATE, fd, 0);
  CHECK(x != MAP_FAILED && mprotect(x, PG, PROT_READ | PROT_EXEC) == 0);
  CHECK(((fn_t)x)() == getpid());
  uint32_t w = 0;
  CHECK(pread(fd, &w, 4, 4) == 4 && w == code[1]);
  munmap(x, PG);
  close(fd);
  unlink(path);
}

static void jit_dual_mapping(void) {
  // ART's JitMemoryRegion: one memfd, a writable data view and an
  // executable code view of the same pages.
  int fd = memfd_create("jit-cache", MFD_CLOEXEC);
  CHECK(fd >= 0 && ftruncate(fd, 4 * PG) == 0);
  uint8_t* rw = mmap(NULL, 4 * PG, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  CHECK(rw != MAP_FAILED);
  rw[3 * PG] = 0x5a;  // data written before the code view exists survives
  // RemapAtEnd: the code view replaces the tail of the data view.
  uint8_t* rx = mmap(rw + 2 * PG, 2 * PG, PROT_READ | PROT_EXEC, MAP_SHARED | MAP_FIXED, fd, 2 * PG);
  CHECK(rx == rw + 2 * PG);
  CHECK(rx[PG] == 0x5a);
  // A second writable view of the code pages.
  uint8_t* w2 = mmap(NULL, 2 * PG, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 2 * PG);
  CHECK(w2 != MAP_FAILED && w2[PG] == 0x5a);
  uint32_t code[2] = {0x52800540 /* mov w0, #42 */, 0xd65f03c0 /* ret */};
  memcpy(w2, code, sizeof code);
  __builtin___clear_cache((char*)rx, (char*)rx + sizeof code);
  CHECK(((fn_t)rx)() == 42);
  code[0] = 0x52800e60; /* mov w0, #115 */
  memcpy(w2, code, 4);
  __builtin___clear_cache((char*)rx, (char*)rx + 4);
  CHECK(((fn_t)rx)() == 115);
  // mprotect to executable also works on a view.
  uint8_t* v3 = mmap(NULL, PG, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 2 * PG);
  CHECK(v3 != MAP_FAILED && mprotect(v3, PG, PROT_READ | PROT_EXEC) == 0);
  CHECK(((fn_t)v3)() == 115);
  // The data view still shares the file's first pages.
  rw[0] = 3;
  uint8_t b = 0;
  CHECK(pread(fd, &b, 1, 0) == 1 && b == 3);
  munmap(v3, PG);
  munmap(w2, 2 * PG);
  munmap(rw, 4 * PG);
  close(fd);
}

int main(void) {
  RUN(mremap_moves);
  RUN(madvise_dontneed);
  RUN(mincore_msync_mlock);
  RUN(membarrier_and_vm_readv);
  RUN(private_file_mappings);
  RUN(jit_dual_mapping);
  DONE();
}
