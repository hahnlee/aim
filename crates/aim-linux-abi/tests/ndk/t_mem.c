// Memory syscalls: mremap, madvise, mincore, msync, mlock, membarrier,
// process_vm_readv, and ART's dual-mapped JIT cache over a memfd.
#include <fcntl.h>
#include <linux/membarrier.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/uio.h>
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
  RUN(jit_dual_mapping);
  DONE();
}
