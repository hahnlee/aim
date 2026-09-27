// P0 experiment 7: 16 KiB pages, MAP_FIXED placement and large reservations.
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/sysctl.h>
#include <time.h>
#include <unistd.h>
#include <signal.h>
#include <sys/wait.h>

static int fails;
#define EXPECT(cond, ...) do { int ok_ = (cond); printf("  [%s] ", ok_ ? " ok " : "FAIL"); printf(__VA_ARGS__); printf("\n"); fails += !ok_; } while (0)
static const char *err(int r) { return r == 0 ? "ok" : strerror(errno); }
static inline uint64_t now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }

static uint64_t region_size(void *p) {
  mach_vm_address_t a = (mach_vm_address_t)p;
  mach_vm_size_t sz = 0;
  vm_region_basic_info_data_64_t info;
  mach_msg_type_number_t cnt = VM_REGION_BASIC_INFO_COUNT_64;
  mach_port_t obj;
  if (mach_vm_region(mach_task_self(), &a, &sz, VM_REGION_BASIC_INFO_64, (vm_region_info_t)&info, &cnt, &obj)) return 0;
  return a == (mach_vm_address_t)p ? sz : 0;
}
static uint64_t footprint(void) {
  task_vm_info_data_t vi;
  mach_msg_type_number_t cnt = TASK_VM_INFO_COUNT;
  task_info(mach_task_self(), TASK_VM_INFO, (task_info_t)&vi, &cnt);
  return vi.phys_footprint;
}

int main(void) {
  setvbuf(stdout, NULL, _IONBF, 0);
  long pg = sysconf(_SC_PAGESIZE);
  int hwpg = 0;
  size_t l = sizeof hwpg;
  sysctlbyname("hw.pagesize", &hwpg, &l, NULL, 0);
  printf("page size: sysconf=%ld getpagesize=%d vm_page_size=%lu hw.pagesize=%d\n", pg, getpagesize(), (unsigned long)vm_page_size, hwpg);

  printf("granularity:\n");
  char *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
  EXPECT(((uintptr_t)p & 16383) == 0 && region_size(p) == 16384, "mmap(len=4096) -> %p, 16 KiB aligned, region is %" PRIu64 " bytes", (void *)p, region_size(p));
  p[8192] = 1;  // the rest of the 16 KiB page is usable (Linux 4 KiB would SIGSEGV here)
  EXPECT(1, "touching byte 8192 of a 4096-byte mapping works (the whole 16 KiB page is mapped)");
  char *q = mmap(NULL, 4 * 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
  int r = mprotect(q + 4096, 4096, PROT_READ);
  EXPECT(r != 0 && errno == EINVAL, "mprotect(addr + 4096, 4096) on a 4 KiB boundary -> %s", err(r));
  r = mprotect(q, 4096, PROT_READ);
  EXPECT(r == 0, "mprotect(16 KiB-aligned addr, 4096) -> %s (rounds the length up to the whole 16 KiB page)", err(r));
  r = munmap(q + 4096, 4096);
  EXPECT(r != 0 && errno == EINVAL, "munmap(addr + 4096, 4096) -> %s", err(r));
  void *fx = mmap(q + 16384 + 4096, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
  EXPECT(fx == MAP_FAILED && errno == EINVAL, "mmap(MAP_FIXED at a 4 KiB-but-not-16 KiB address) -> %s", fx == MAP_FAILED ? strerror(errno) : "mapped");

  char path[] = "/tmp/p0pagesXXXXXX";
  int fd = mkstemp(path);
  unlink(path);
  static char data[65536];
  for (int i = 0; i < 65536; i++) data[i] = (char)(i / 4096);
  (void)write(fd, data, sizeof data);
  void *f4 = mmap(NULL, 4096, PROT_READ, MAP_PRIVATE, fd, 4096);
  EXPECT(f4 == MAP_FAILED && errno == EINVAL, "file mmap at offset 4096 -> %s (a 4 KiB-aligned ELF segment cannot be mmapped in place)",
         f4 == MAP_FAILED ? strerror(errno) : "mapped");
  char *f16 = mmap(NULL, 16384, PROT_READ, MAP_PRIVATE, fd, 16384);
  EXPECT(f16 != MAP_FAILED && f16[0] == 4, "file mmap at offset 16384 -> ok, first byte from file page 4");

  printf("MAP_FIXED semantics (ELF segment placement):\n");
  char *seg = mmap(NULL, 8 * 16384, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);  // linker64 reserves the whole image first
  // How can a guest .so's text become executable? Darwin requires code
  // signatures for executable file-backed pages; an ELF file has none.
  char *t1 = mmap(seg, 2 * 16384, PROT_READ | PROT_EXEC, MAP_PRIVATE | MAP_FIXED, fd, 0);
  printf("  text: mmap(file, PROT_READ|PROT_EXEC, MAP_PRIVATE|MAP_FIXED): %s\n", t1 == MAP_FAILED ? strerror(errno) : "ok");
  char *t2 = mmap(seg, 2 * 16384, PROT_READ, MAP_PRIVATE | MAP_FIXED, fd, 0);
  r = t2 == MAP_FAILED ? -1 : mprotect(t2, 2 * 16384, PROT_READ | PROT_EXEC);
  printf("  text: mmap(file, PROT_READ) then mprotect(+PROT_EXEC): %s\n", err(r));
  char *t3 = mmap(seg, 2 * 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_FIXED, fd, 0);
  for (int i = 0; t3 != MAP_FAILED && i < 2 * 16384; i += 16384) t3[i] = t3[i];  // force copy-on-write
  r = t3 == MAP_FAILED ? -1 : mprotect(t3, 2 * 16384, PROT_READ | PROT_EXEC);
  printf("  text: mmap(file, PROT_READ|PROT_WRITE, MAP_PRIVATE), COW every page, mprotect(RX): %s\n", err(r));
  char *t4 = mmap(seg, 2 * 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
  r = (t4 == MAP_FAILED || pread(fd, t4, 2 * 16384, 0) != 2 * 16384) ? -1 : mprotect(t4, 2 * 16384, PROT_READ | PROT_EXEC);
  printf("  text: anonymous MAP_FIXED + pread + mprotect(RX): %s\n", err(r));
  char *text = r == 0 ? t4 : t1;
  char *dat = mmap(seg + 2 * 16384, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_FIXED, fd, 2 * 16384);
  char *bss = mmap(seg + 3 * 16384, 2 * 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
  EXPECT(text == seg && dat == seg + 2 * 16384 && bss == seg + 3 * 16384 && text[16384] == 4 && dat[0] == 8 && bss[0] == 0,
         "RX text + file-backed RW data + anon bss placed MAP_FIXED over a PROT_NONE reservation");
  dat[1] = 0x55;
  char *again = mmap(seg + 2 * 16384, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
  EXPECT(again == seg + 2 * 16384 && again[1] == 0, "MAP_FIXED over an existing mapping replaces it (Linux semantics)");
  mach_vm_address_t want = (mach_vm_address_t)(seg + 16384);
  kern_return_t kr = mach_vm_allocate(mach_task_self(), &want, 16384, VM_FLAGS_FIXED);
  EXPECT(kr == KERN_NO_SPACE, "MAP_FIXED_NOREPLACE (absent on Darwin) is expressible: mach_vm_allocate(VM_FLAGS_FIXED) on an occupied range -> %s",
         mach_error_string(kr));
  void *hint = mmap(seg + 16384, 16384, PROT_READ, MAP_PRIVATE | MAP_ANON, -1, 0);
  EXPECT(hint != seg + 16384, "a hint without MAP_FIXED on an occupied range is moved elsewhere (%p)", hint);

  // Do the executable variants actually run? Code-signing enforcement
  // happens at page-in, so a successful mprotect proves nothing.
  {
    char cpath[] = "/tmp/p0codeXXXXXX";
    int cfd = mkstemp(cpath);
    unlink(cpath);
    static uint32_t code[4096];
    code[0] = 0x52800000u | (42u << 5);  // mov w0, #42
    code[1] = 0xd65f03c0u;               // ret
    (void)write(cfd, code, sizeof code);
    const char *names[] = {"file PROT_READ -> mprotect RX", "file MAP_PRIVATE RW, COW, mprotect RX", "anonymous + pread + mprotect RX"};
    for (int k = 0; k < 3; k++) {
      pid_t pid = fork();
      if (pid == 0) {
        char *m;
        if (k == 0) {
          m = mmap(NULL, 16384, PROT_READ, MAP_PRIVATE, cfd, 0);
        } else if (k == 1) {
          m = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE, cfd, 0);
          m[64] = m[64];
        } else {
          m = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
          pread(cfd, m, 16384, 0);
        }
        if (mprotect(m, 16384, PROT_READ | PROT_EXEC)) _exit(2);
        _exit(((int (*)(void))m)() == 42 ? 0 : 3);
      }
      int st;
      waitpid(pid, &st, 0);
      int ok = WIFEXITED(st) && WEXITSTATUS(st) == 0;
      printf("  %s executing text via %s: %s\n", ok ? "[ ok ]" : "[info]", names[k],
             ok ? "runs" : WIFSIGNALED(st) ? (WTERMSIG(st) == SIGKILL ? "killed (SIGKILL, code signing)" : strsignal(WTERMSIG(st))) : "failed");
      if (k == 2) fails += !ok;
    }
    close(cfd);
  }

  printf("large guest reservations:\n");
  uint64_t fp0 = footprint();
  uint64_t sizes[] = {64ull << 30, 1ull << 40, 16ull << 40, 64ull << 40, 100ull << 40};
  for (int i = 0; i < 5; i++) {
    uint64_t t0 = now_ns();
    char *res = mmap(NULL, sizes[i], PROT_NONE, MAP_PRIVATE | MAP_ANON | MAP_NORESERVE, -1, 0);
    uint64_t dt = now_ns() - t0;
    if (res == MAP_FAILED) {
      printf("  reserve %6.0f GiB PROT_NONE: FAILED (%s)\n", sizes[i] / (double)(1ull << 30), strerror(errno));
      continue;
    }
    // Place a few "segments" deep inside, touch them, then release.
    uint64_t offs[] = {0, sizes[i] / 2, sizes[i] - (1 << 20)};
    int ok = 1;
    for (int k = 0; k < 3; k++) {
      char *at = res + (offs[k] & ~16383ull);
      char *m = mmap(at, 1 << 20, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
      ok &= m == at;
      if (m == at) memset(m, 1, 1 << 20);
    }
    uint64_t fp1 = footprint();
    printf("  [%s] reserve %6.0f GiB PROT_NONE at %p in %.1f us; MAP_FIXED 3 x 1 MiB inside and touched; footprint +%.1f MiB\n",
           ok ? " ok " : "FAIL", sizes[i] / (double)(1ull << 30), (void *)res, dt / 1e3, (fp1 - fp0) / 1048576.0);
    fails += !ok;
    munmap(res, sizes[i]);
  }

  printf("low 4 GiB (ART's heap, 32-bit compressed references, lives below 4 GiB on arm64):\n");
  mach_vm_address_t a = 0;
  mach_vm_size_t sz = 0;
  vm_region_basic_info_data_64_t info;
  mach_msg_type_number_t cnt = VM_REGION_BASIC_INFO_COUNT_64;
  mach_port_t obj;
  kr = mach_vm_region(mach_task_self(), &a, &sz, VM_REGION_BASIC_INFO_64, (vm_region_info_t)&info, &cnt, &obj);
  printf("  lowest region: %#llx + %#llx (prot %d)\n", a, sz, info.protection);
  uint64_t lows[] = {0x10000, 0x1000000, 0x40000000, 0xc0000000};
  for (int i = 0; i < 4; i++) {
    void *m = mmap((void *)lows[i], 1 << 20, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
    printf("  MAP_FIXED 1 MiB at %#llx: %s\n", lows[i], m == MAP_FAILED ? strerror(errno) : "ok");
    if (m != MAP_FAILED) munmap(m, 1 << 20);
  }
  void *h = mmap((void *)0x40000000, 1 << 20, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
  printf("  hint 0x40000000 without MAP_FIXED -> %p\n", h);
  printf("%s (%d failures)\n", fails ? "FAIL" : "PASS", fails);
  return fails != 0;
}
