#include "darwin_art_bionic_fortify_io.h"
#include <signal.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

static int rejects(size_t count, size_t capacity) {
  pid_t child = fork();
  if (child < 0) return 0;
  if (child == 0) {
    struct rlimit limit = {0, 0};
    if (setrlimit(RLIMIT_CORE, &limit) != 0) _exit(2);
    darwin_art_bionic_check_io_buffer(count, capacity);
    _exit(0);
  }
  int status;
  if (waitpid(child, &status, 0) != child) return 0;
  return WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT;
}
int main(void) {
  darwin_art_bionic_check_io_buffer(0, 0);
  darwin_art_bionic_check_io_buffer(8, 8);
  darwin_art_bionic_check_io_buffer(INT64_MAX, SIZE_MAX);
  return rejects(9, 8) && rejects((size_t)INT64_MAX + 1, SIZE_MAX) ? 0 : 1;
}
