#include "aim_bionic_abort.h"

#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

enum {
  kThreadCount = 8,
  kHostErrno = 31871,
};

static void Fail(const char* message) {
  fprintf(stderr, "bionic-abort-differential: %s\n", message);
  exit(1);
}

static void CheckMessage(const char* expected) {
  const AimBionicAbortMessage* message =
      aim_bionic_abort_message_for_test();
  if (message == NULL) Fail("missing abort message");
  const unsigned char* allocation = (const unsigned char*)message - 16;
  uint64_t magic1;
  uint64_t magic2;
  memcpy(&magic1, allocation, sizeof(magic1));
  memcpy(&magic2, allocation + 8, sizeof(magic2));
  const size_t expected_size = 24 + strlen(expected) + 1;
  if (magic1 != UINT64_C(0xb18e40886ac388f0) ||
      magic2 != UINT64_C(0xc6dfba755a1de0b5) ||
      message->size != expected_size ||
      memcmp(message->message, expected, strlen(expected) + 1) != 0) {
    Fail("abort message bytes/layout drift");
  }
}

static void RunMessageChild(const char* first, const char* second) {
  const pid_t child = fork();
  if (child == -1) Fail("fork message child");
  if (child == 0) {
    errno = kHostErrno;
    aim_bionic_android_set_abort_message(first);
    const AimBionicAbortMessage* original =
        aim_bionic_abort_message_for_test();
    aim_bionic_android_set_abort_message(second);
    if (aim_bionic_abort_message_for_test() != original ||
        errno != kHostErrno) {
      _exit(2);
    }
    CheckMessage(first == NULL ? "(null)" : first);
    _exit(0);
  }
  int status = 0;
  if (waitpid(child, &status, 0) != child || !WIFEXITED(status) ||
      WEXITSTATUS(status) != 0) {
    Fail("message child failed");
  }
}

static const char* const kThreadMessages[kThreadCount] = {
    "thread-message-0", "thread-message-1", "thread-message-2",
    "thread-message-3", "thread-message-4", "thread-message-5",
    "thread-message-6", "thread-message-7",
};

static void* SetThreadMessage(void* argument) {
  const uintptr_t index = (uintptr_t)argument;
  aim_bionic_android_set_abort_message(kThreadMessages[index]);
  return NULL;
}

static void RunConcurrencyChild(void) {
  const pid_t child = fork();
  if (child == -1) Fail("fork concurrency child");
  if (child == 0) {
    pthread_t threads[kThreadCount];
    for (uintptr_t index = 0; index < kThreadCount; ++index) {
      if (pthread_create(&threads[index], NULL, SetThreadMessage,
                         (void*)index) != 0) {
        _exit(3);
      }
    }
    for (size_t index = 0; index < kThreadCount; ++index) {
      if (pthread_join(threads[index], NULL) != 0) _exit(4);
    }
    const AimBionicAbortMessage* winner =
        aim_bionic_abort_message_for_test();
    if (winner == NULL) _exit(5);
    int matched = 0;
    for (size_t index = 0; index < kThreadCount; ++index) {
      if (strcmp(winner->message, kThreadMessages[index]) == 0) {
        CheckMessage(kThreadMessages[index]);
        matched = 1;
      }
    }
    if (!matched) _exit(6);
    aim_bionic_android_set_abort_message("late-message");
    if (aim_bionic_abort_message_for_test() != winner) _exit(7);
    _exit(0);
  }
  int status = 0;
  if (waitpid(child, &status, 0) != child || !WIFEXITED(status) ||
      WEXITSTATUS(status) != 0) {
    Fail("message concurrency failed");
  }
}

static void ReturningHandler(int signal_number) {
  (void)signal_number;
}

static void RunDeathCase(int mode) {
  const pid_t child = fork();
  if (child == -1) Fail("fork death child");
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
    aim_bionic_abort();
  }
  int status = 0;
  if (waitpid(child, &status, 0) != child || !WIFSIGNALED(status) ||
      WTERMSIG(status) != SIGABRT) {
    Fail("abort child did not die from SIGABRT");
  }
}

int main(int argc, char** argv) {
  RunMessageChild("first-message", "second-message");
  RunMessageChild(NULL, "second-message");
  RunConcurrencyChild();
  if (argc == 1) {
    for (int mode = 0; mode < 4; ++mode) RunDeathCase(mode);
  } else if (argc != 2 || strcmp(argv[1], "--messages-only") != 0) {
    return 2;
  }
  if (argc == 1) {
    puts("bionic-abort-differential: PASS message=first-wins+null+magic+size threads=8 death=default+blocked+ignored+returning host-errno=preserved");
  } else {
    puts("bionic-abort-differential: PASS message=first-wins+null+magic+size threads=8 death=skipped-for-sanitizer host-errno=preserved");
  }
  return 0;
}
