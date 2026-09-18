#include "darwin_art_bionic_process_state.h"

#include <execinfo.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

// Android's exit ABI terminates the real host process. Diagnostics must never
// turn this boundary into an ART shutdown, a service callback, or a retry.
static volatile sig_atomic_t g_exit_backtrace_enabled;

// Environment lookup belongs to safe library initialization, never _exit.
__attribute__((constructor)) static void InitializeExitDiagnostics(void) {
  g_exit_backtrace_enabled =
      getenv("DARWIN_ART_DEBUG_PROCESS_EXIT_BACKTRACE") != NULL;
}

static char* AppendText(char* output, const char* text) {
  while (*text != '\0') *output++ = *text++;
  return output;
}

static char* AppendUnsigned(char* output, uint64_t value, unsigned base) {
  static const char digits[] = "0123456789abcdef";
  char reversed[32];
  size_t count = 0;
  do {
    reversed[count++] = digits[value % base];
    value /= base;
  } while (value != 0);
  while (count != 0) *output++ = reversed[--count];
  return output;
}

static void LogExit(const char* name, int status, void* caller) {
  char message[160];
  char* output = AppendText(message, "DARWIN Bionic ");
  output = AppendText(output, name);
  output = AppendText(output, " pid=");
  output = AppendUnsigned(output, (uint64_t)getpid(), 10);
  output = AppendText(output, " status=");
  const int64_t signed_status = status;
  if (signed_status < 0) *output++ = '-';
  output = AppendUnsigned(output,
      (uint64_t)(signed_status < 0 ? -signed_status : signed_status), 10);
  output = AppendText(output, " caller=0x");
  output = AppendUnsigned(output, (uintptr_t)caller, 16);
  *output++ = '\n';
  // Fixed literals, at most 20 decimal/16 hex digits: fits the stack buffer.
  (void)write(STDERR_FILENO, message, (size_t)(output - message));
  // Explicit opt-in only: backtrace is not async-signal-safe, so normal _exit
  // callers (including signal handlers) never enter the host unwinder.
  if (g_exit_backtrace_enabled) {
    void* frames[24];
    const int count = backtrace(frames, 24);
    for (int index = 0; index < count; ++index) {
      const int frame_length = snprintf(message, sizeof(message),
          "DARWIN Bionic exit-frame pid=%d depth=%d pc=%p\n",
          getpid(), index, frames[index]);
      if (frame_length > 0 && (size_t)frame_length < sizeof(message)) {
        (void)write(STDERR_FILENO, message, (size_t)frame_length);
      }
    }
  }
}

void darwin_art_bionic_exit(int status) {
  LogExit("exit", status, __builtin_return_address(0));
  exit(status);
}

void darwin_art_bionic__exit(int status) {
  LogExit("_exit", status, __builtin_return_address(0));
  _exit(status);
}
