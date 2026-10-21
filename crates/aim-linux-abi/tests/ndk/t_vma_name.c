// Anonymous mapping names (prctl PR_SET_VMA_ANON_NAME, mm/madvise.c) as
// /proc/self/maps and smaps show them (fs/proc/task_mmu.c get_vma_name).
#include <fcntl.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

#ifndef PR_SET_VMA
#define PR_SET_VMA 0x53564d41
#define PR_SET_VMA_ANON_NAME 0
#endif

static long pg;

static int name(void* p, size_t len, const char* n) {
  return prctl(PR_SET_VMA, PR_SET_VMA_ANON_NAME, (unsigned long)p, len, (unsigned long)n);
}

// The whole of /proc/self/<file>, NUL-terminated.
static char* slurp(const char* file) {
  char path[64];
  snprintf(path, sizeof path, "/proc/self/%s", file);
  int fd = open(path, O_RDONLY | O_CLOEXEC);
  if (fd < 0) return NULL;
  size_t cap = 1 << 16, len = 0;
  char* buf = malloc(cap);
  ssize_t n;
  while ((n = read(fd, buf + len, cap - len - 1)) > 0) {
    len += n;
    if (cap - len < 4096) buf = realloc(buf, cap *= 2);
  }
  close(fd);
  buf[len] = 0;
  return buf;
}

// The name `file` shows for the mapping [lo, hi), "" for none, or NULL
// when no mapping starts and ends there.
static char* shown(const char* file, void* lo, void* hi) {
  static char out[128];
  char* all = slurp(file);
  if (!all) return NULL;
  char key[48];
  snprintf(key, sizeof key, "%08lx-%08lx ", (unsigned long)lo, (unsigned long)hi);
  char* line = all;
  char* found = NULL;
  while (line && *line) {
    if (strncmp(line, key, strlen(key)) == 0) {
      char* end = strchr(line, '\n');
      if (end) *end = 0;
      // The name starts after the inode, past the padding.
      char* p = line;
      for (int field = 0; field < 5 && p; field++) {
        p = strchr(p, ' ');
        if (p) p++;
      }
      while (p && *p == ' ') p++;
      snprintf(out, sizeof out, "%s", p ? p : "");
      found = out;
      break;
    }
    line = strchr(line, '\n');
    if (line) line++;
  }
  free(all);
  return found;
}

static void names_cut_mappings(void) {
  char* p = mmap(NULL, 4 * pg, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(p != MAP_FAILED);
  CHECK(name(p + pg, 2 * pg, "aim test") == 0);
  char* n = shown("maps", p + pg, p + 3 * pg);
  CHECK(n && strcmp(n, "[anon:aim test]") == 0);
  CHECK((n = shown("maps", p, p + pg)) && strcmp(n, "") == 0);
  CHECK((n = shown("maps", p + 3 * pg, p + 4 * pg)) && strcmp(n, "") == 0);
  CHECK((n = shown("smaps", p + pg, p + 3 * pg)) && strcmp(n, "[anon:aim test]") == 0);
  // The same name again over the rest merges the mapping whole.
  CHECK(name(p, 4 * pg, "aim test") == 0);
  CHECK((n = shown("maps", p, p + 4 * pg)) && strcmp(n, "[anon:aim test]") == 0);
  // A partial munmap leaves both ends named.
  CHECK(munmap(p + pg, pg) == 0);
  CHECK((n = shown("maps", p, p + pg)) && strcmp(n, "[anon:aim test]") == 0);
  CHECK((n = shown("maps", p + 2 * pg, p + 4 * pg)) && strcmp(n, "[anon:aim test]") == 0);
  // A null name clears it.
  CHECK(name(p, pg, NULL) == 0);
  CHECK((n = shown("maps", p, p + pg)) && strcmp(n, "") == 0);
  munmap(p, 4 * pg);
}

static void mremap_keeps_the_name(void) {
  char* p = mmap(NULL, 2 * pg, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(p != MAP_FAILED);
  CHECK(name(p, 2 * pg, "moved") == 0);
  char* q = mremap(p, 2 * pg, 3 * pg, MREMAP_MAYMOVE);
  CHECK(q != MAP_FAILED);
  char* n = shown("maps", q, q + 3 * pg);
  CHECK(n && strcmp(n, "[anon:moved]") == 0);
  // A new mapping in its place has no name.
  char* r = mmap(q, pg, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
  CHECK(r == q);
  CHECK((n = shown("maps", q, q + pg)) && strcmp(n, "") == 0);
  munmap(q, 3 * pg);
}

static void fork_child_keeps_names(void) {
  char* p = mmap(NULL, pg, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(p != MAP_FAILED);
  CHECK(name(p, pg, "inherited") == 0);
  FORK_OR_SKIP(child);
  if (child == 0) {
    char* n = shown("maps", p, p + pg);
    _exit(n && strcmp(n, "[anon:inherited]") == 0 ? 0 : 1);
  }
  int status;
  CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  munmap(p, pg);
}

static void refusals(void) {
  char* p = mmap(NULL, pg, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(p != MAP_FAILED);
  char longest[80], too_long[81];
  memset(longest, 'a', sizeof longest - 1);
  longest[sizeof longest - 1] = 0;
  memset(too_long, 'a', sizeof too_long - 1);
  too_long[sizeof too_long - 1] = 0;
  CHECK(name(p, pg, longest) == 0);
  errno = 0;
  CHECK(name(p, pg, too_long) == -1 && errno == EINVAL);
  errno = 0;
  CHECK(name(p, pg, "a[b") == -1 && errno == EINVAL);
  errno = 0;
  CHECK(name(p + 1, pg, "x") == -1 && errno == EINVAL);
  errno = 0;
  CHECK(prctl(PR_SET_VMA, 1, (unsigned long)p, pg, (unsigned long)"x") == -1 && errno == EINVAL);
  errno = 0;
  CHECK(name(p, pg, (const char*)8) == -1 && errno == EFAULT);
  CHECK(name(p, 0, "x") == 0);
  // A hole: what is mapped is named, then ENOMEM.
  CHECK(munmap(p, pg) == 0);
  errno = 0;
  CHECK(name(p, pg, "x") == -1 && errno == ENOMEM);
  // A file mapping takes no name.
  int fd = open("/proc/self/exe", O_RDONLY | O_CLOEXEC);
  CHECK(fd >= 0);
  void* f = mmap(NULL, pg, PROT_READ, MAP_PRIVATE, fd, 0);
  close(fd);
  CHECK(f != MAP_FAILED);
  errno = 0;
  CHECK(name(f, pg, "x") == -1 && errno == EBADF);
  munmap(f, pg);
}

int main(void) {
  pg = sysconf(_SC_PAGESIZE);
  RUN(names_cut_mappings);
  RUN(mremap_keeps_the_name);
  RUN(fork_child_keeps_names);
  RUN(refusals);
  DONE();
}
