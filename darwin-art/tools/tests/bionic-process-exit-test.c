#include "darwin_art_bionic_process_state.h"

#include <assert.h>
#include <stdio.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static int marker_fd;
static const char* executable;
static void AtExit(void) { assert(write(marker_fd, "X", 1) == 1); }
static void ExitSignal(int signal_number) {
  (void)signal_number;
  darwin_art_bionic__exit(37);
}

static void Check(int immediate, int diagnostics) {
  int marker[2];
  assert(pipe(marker) == 0);
  const pid_t child = fork();
  assert(child >= 0);
  if (child == 0) {
    close(marker[0]);
    marker_fd = marker[1];
    assert(atexit(AtExit) == 0);
    if (diagnostics) {
      assert(setenv("DARWIN_ART_DEBUG_PROCESS_EXIT_BACKTRACE", "1", 1) == 0);
    } else {
      assert(unsetenv("DARWIN_ART_DEBUG_PROCESS_EXIT_BACKTRACE") == 0);
    }
    char descriptor[24];
    // Configure the environment before exec: diagnostics are initialized at
    // library load, not by consulting mutable environment in the exit path.
    assert(snprintf(descriptor, sizeof(descriptor), "%d", marker_fd) > 0);
    const char* mode = immediate == 2 ? "signal" :
        (immediate ? "immediate" : "normal");
    execl(executable, executable, mode,
          descriptor, (char*)NULL);
    _exit(127);
  }
  close(marker[1]);
  int status;
  assert(waitpid(child, &status, 0) == child);
  assert(WIFEXITED(status) && WEXITSTATUS(status) == 37);
  char byte = 0;
  assert(read(marker[0], &byte, 1) == (immediate ? 0 : 1));
  if (!immediate) assert(byte == 'X');
  close(marker[0]);
}

int main(int argc, char** argv) {
  if (argc == 3) {
    marker_fd = atoi(argv[2]);
    assert(atexit(AtExit) == 0);
    if (strcmp(argv[1], "signal") == 0) {
      assert(signal(SIGUSR1, ExitSignal) != SIG_ERR);
      assert(raise(SIGUSR1) == 0);
      return 99;
    }
    if (strcmp(argv[1], "immediate") == 0) darwin_art_bionic__exit(37);
    darwin_art_bionic_exit(37);
  }
  executable = argv[0];
  Check(0, 0);
  Check(1, 0);
  Check(0, 1);
  Check(1, 1);
  Check(2, 0);
  return 0;
}
