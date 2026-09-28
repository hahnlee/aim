#include "aim_bionic_abort.h"

#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

extern int aim_bionic_errno_set_from_darwin(int darwin_errno);

_Static_assert(SIGABRT == 6,
               "Darwin SIGABRT must equal pinned Android arm64 SIGABRT");
_Static_assert(sizeof(size_t) == 8, "Android arm64 size_t width drift");

typedef struct MagicAbortMessage {
  uint64_t magic1;
  uint64_t magic2;
  size_t size;
  char message[];
} MagicAbortMessage;

_Static_assert(offsetof(AimBionicAbortMessage, message) == sizeof(size_t),
               "abort message layout drift");
_Static_assert(offsetof(MagicAbortMessage, size) == 2 * sizeof(uint64_t),
               "magic abort message layout drift");
_Static_assert(sizeof(MagicAbortMessage) == 24,
               "magic abort message size drift");

static pthread_mutex_t gAbortMessageLock = PTHREAD_MUTEX_INITIALIZER;
static AimBionicAbortMessage* gAbortMessage;

__attribute__((optnone)) static void FillAbortMessageMagic(
    MagicAbortMessage* allocation) {
  allocation->magic1 = UINT64_C(0xb18e40886ac388f0);
  allocation->magic2 = UINT64_C(0xc6dfba755a1de0b5);
}

__attribute__((noreturn)) void aim_bionic_abort(void) {
  static const char kMarker[] = "DARWIN Bionic abort entered\n";
  (void)write(STDERR_FILENO, kMarker, sizeof(kMarker) - 1);
  const uintptr_t* frame = (const uintptr_t*)__builtin_frame_address(0);
  const uintptr_t* caller_frame =
      frame != NULL ? (const uintptr_t*)frame[0] : NULL;
  void* parent = caller_frame != NULL ? (void*)caller_frame[1] : NULL;
  fprintf(stderr, "DARWIN Bionic abort: caller=%p parent=%p\n",
          __builtin_return_address(0), parent);
  const uintptr_t* walk = frame;
  for (size_t depth = 0; depth < 16 && walk != NULL; ++depth) {
    fprintf(stderr, "DARWIN Bionic abort frame[%zu]=%p fp=%p\n", depth,
            (void*)walk[1], (const void*)walk);
    const uintptr_t* next = (const uintptr_t*)walk[0];
    if (next <= walk || (uintptr_t)next - (uintptr_t)walk > 1024 * 1024) {
      break;
    }
    walk = next;
  }
  sigset_t mask;
  sigfillset(&mask);
  sigdelset(&mask, SIGABRT);

  (void)pthread_sigmask(SIG_SETMASK, &mask, NULL);
  (void)pthread_kill(pthread_self(), SIGABRT);

  struct sigaction action;
  memset(&action, 0, sizeof(action));
  action.sa_handler = SIG_DFL;
  action.sa_flags = SA_RESTART;
  sigemptyset(&action.sa_mask);
  (void)sigaction(SIGABRT, &action, NULL);

  (void)pthread_sigmask(SIG_SETMASK, &mask, NULL);
  (void)pthread_kill(pthread_self(), SIGABRT);
  _exit(127);
}

__attribute__((noreturn)) void aim_bionic___stack_chk_fail(void) {
  fprintf(stderr, "DARWIN Bionic stack check failure: caller=%p\n",
          __builtin_return_address(0));
  aim_bionic_abort();
}

__attribute__((noreturn)) static void AimBionicAssert2(
    const char* file, int line, const char* function, const char* expression) {
  fprintf(stderr, "%s:%d: %s: assertion \"%s\" failed\n",
          file != NULL ? file : "<unknown>", line,
          function != NULL ? function : "<unknown>",
          expression != NULL ? expression : "<unknown>");
  aim_bionic_abort();
}

__attribute__((noreturn)) static void AimBionicAssert(
    const char* file, int line, const char* expression) {
  AimBionicAssert2(file, line, "<unknown>", expression);
}

void aim_bionic_android_set_abort_message(const char* message) {
  const int saved_errno = errno;
  (void)pthread_mutex_lock(&gAbortMessageLock);
  if (gAbortMessage != NULL) {
    (void)pthread_mutex_unlock(&gAbortMessageLock);
    errno = saved_errno;
    return;
  }
  if (message == NULL) message = "(null)";

  const size_t size = sizeof(MagicAbortMessage) + strlen(message) + 1;
  void* mapping = mmap(NULL, size, PROT_READ | PROT_WRITE,
                       MAP_ANON | MAP_PRIVATE, -1, 0);
  if (mapping == MAP_FAILED) {
    const int map_errno = errno;
    (void)aim_bionic_errno_set_from_darwin(map_errno);
    (void)pthread_mutex_unlock(&gAbortMessageLock);
    errno = saved_errno;
    return;
  }

  MagicAbortMessage* allocation = (MagicAbortMessage*)mapping;
  FillAbortMessageMagic(allocation);
  allocation->size = size;
  memcpy(allocation->message, message, strlen(message) + 1);
  gAbortMessage = (AimBionicAbortMessage*)&allocation->size;
  (void)pthread_mutex_unlock(&gAbortMessageLock);
  errno = saved_errno;
}

const AimBionicAbortMessage*
aim_bionic_abort_message_for_test(void) {
  (void)pthread_mutex_lock(&gAbortMessageLock);
  const AimBionicAbortMessage* result = gAbortMessage;
  (void)pthread_mutex_unlock(&gAbortMessageLock);
  return result;
}

void* aim_bionic_abort_resolve(const char* soname,
                                      const char* symbol,
                                      const char* version) {
  if (soname == NULL || symbol == NULL || version == NULL ||
      strcmp(soname, "libc.so") != 0 || strcmp(version, "LIBC") != 0)
    return NULL;
  if (strcmp(symbol, "abort") == 0)
    return (void*)(uintptr_t)&aim_bionic_abort;
  if (strcmp(symbol, "__stack_chk_fail") == 0)
    return (void*)(uintptr_t)&aim_bionic___stack_chk_fail;
  if (strcmp(symbol, "__assert") == 0)
    return (void*)(uintptr_t)&AimBionicAssert;
  if (strcmp(symbol, "__assert2") == 0)
    return (void*)(uintptr_t)&AimBionicAssert2;
  if (strcmp(symbol, "android_set_abort_message") == 0)
    return (void*)(uintptr_t)&aim_bionic_android_set_abort_message;
  return NULL;
}

int aim_bionic_abort_capability(const char* capability) {
  if (capability == NULL) return 0;
  return strcmp(capability, "SIGABRT=6-raw-forwarding") == 0 ||
         strcmp(capability, "abort-message-first-wins") == 0 ||
         strcmp(capability, "abort-message-process-lifetime") == 0 ||
         strcmp(capability, "abort-message-magic-layout") == 0;
}
