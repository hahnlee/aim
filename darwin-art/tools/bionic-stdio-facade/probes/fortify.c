#include "darwin_art_bionic_stdio.h"
#include <assert.h>
#include <signal.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

// Test doubles verify delegation; real stream I/O belongs to separate tests.
size_t darwin_art_bionic_fread(void* p, size_t s, size_t n, DarwinArtAndroidFile* f) {
  (void)p; (void)s; (void)n; (void)f; return 41;
}
size_t darwin_art_bionic_fwrite(const void* p, size_t s, size_t n, DarwinArtAndroidFile* f) {
  (void)p; (void)s; (void)n; (void)f; return 42;
}
static void dies(int writing) {
  pid_t pid = fork();
  assert(pid >= 0);
  if (pid == 0) {
    struct rlimit limit = {0, 0};
    if (setrlimit(RLIMIT_CORE, &limit) != 0) _exit(2);
    if (writing) darwin_art_bionic___fwrite_chk(NULL, 2, 3, NULL, 5);
    else darwin_art_bionic___fread_chk(NULL, 2, 3, NULL, 5);
    _exit(0);
  }
  int status;
  assert(waitpid(pid, &status, 0) == pid);
  assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
}
int main(void) {
  assert(darwin_art_bionic___fread_chk(NULL, 0, SIZE_MAX, NULL, 0) == 41);
  assert(darwin_art_bionic___fwrite_chk(NULL, 2, 3, NULL, 6) == 42);
  assert(darwin_art_bionic___fread_chk(NULL, SIZE_MAX, 2, NULL, 0) == 41);
  assert(darwin_art_bionic___fwrite_chk(NULL, SIZE_MAX, 2, NULL, 0) == 42);
  dies(0); dies(1);
}
