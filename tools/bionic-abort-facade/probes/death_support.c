#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

static void ReturningHandler(int signal_number) {
  (void)signal_number;
}

int aim_abort_probe_death(uintptr_t function_address, int mode) {
  const pid_t child = fork();
  if (child == -1) return 0;
  if (child == 0) {
    struct sigaction action;
    sigemptyset(&action.sa_mask);
    action.sa_flags = 0;
    action.sa_handler = SIG_DFL;
    if (mode == 1) {
      sigset_t blocked;
      sigemptyset(&blocked);
      sigaddset(&blocked, SIGABRT);
      (void)pthread_sigmask(SIG_BLOCK, &blocked, NULL);
    } else if (mode == 2) {
      action.sa_handler = SIG_IGN;
      (void)sigaction(SIGABRT, &action, NULL);
    } else if (mode == 3) {
      action.sa_handler = ReturningHandler;
      (void)sigaction(SIGABRT, &action, NULL);
    }
    void (*function)(void) = (void (*)(void))function_address;
    function();
    _exit(126);
  }
  int status = 0;
  if (waitpid(child, &status, 0) != child) return 0;
  return WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT;
}
