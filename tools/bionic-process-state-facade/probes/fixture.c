#include <asm/hwcap.h>
#include <errno.h>
#include <libgen.h>
#include <signal.h>
#include <stdlib.h>
#include <sys/auxv.h>
#include <sys/random.h>
#include <sys/system_properties.h>

static const char* gEnvironment;
static const unsigned char* gRandom;
static volatile sig_atomic_t gSignalSeen;

static void LegacySignalHandler(int signal_number) {
  if (signal_number == SIGUSR1) ++gSignalSeen;
}

static void SigactionHandler(int signal_number) {
  if (signal_number == SIGUSR2) ++gSignalSeen;
}

static void RecoverFaultHandler(int signal_number, siginfo_t* info,
                                void* raw_context) {
  (void)info;
  if (signal_number != SIGSEGV || raw_context == NULL) return;
  ucontext_t* context = (ucontext_t*)raw_context;
  context->uc_mcontext.pc += 4;
  ++gSignalSeen;
}

static int Equal(const char* left, const char* right) {
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return *left == *right;
}

__attribute__((visibility("default"))) int bionic_process_fixture_basic(void) {
  gEnvironment = getenv("ANDROID_ROOT");
  if (gEnvironment == NULL || !Equal(gEnvironment, "/system") ||
      getenv("ANDROID_ROOT") != gEnvironment) return 1;
  errno = 777;
  if (getenv("HOME") != NULL || getenv("HOST_SECRET") != NULL || errno != 777)
    return 2;

  char value[PROP_VALUE_MAX];
  int length = __system_property_get("ro.build.version.sdk", value);
  if (length != 2 || !Equal(value, "36")) return 3;
  length = __system_property_get("test.boundary", value);
  if (length != PROP_VALUE_MAX - 1 || value[0] != 'X' ||
      value[PROP_VALUE_MAX - 2] != 'X' || value[PROP_VALUE_MAX - 1] != '\0')
    return 4;
  value[0] = 'Q';
  errno = 778;
  if (__system_property_get("missing.property", value) != 0 || value[0] != '\0' ||
      errno != 778) return 5;

  if (getauxval(AT_PAGESZ) != 16384 ||
      getauxval(AT_HWCAP) != (HWCAP_FP | HWCAP_ASIMD)) return 6;
  errno = 779;
  if (getauxval(AT_HWCAP2) != 0 || getauxval(AT_SECURE) != 0 || errno != 779)
    return 7;
  gRandom = (const unsigned char*)getauxval(AT_RANDOM);
  if (gRandom == NULL || gRandom[0] != 0x10 || gRandom[15] != 0x0f ||
      (const unsigned char*)getauxval(AT_RANDOM) != gRandom) return 8;
  errno = 0;
  if (getauxval(0x7fffffffUL) != 0 || errno != ENOENT) return 9;
  const char basename_input[] = "/system/lib/libgrap.so///";
  char* basename_result = basename(basename_input);
  if (basename_result == NULL || !Equal(basename_result, "libgrap.so") ||
      !Equal(basename_input, "/system/lib/libgrap.so///"))
    return 10;
  if (!Equal(basename(NULL), ".") || !Equal(basename("////"), "/"))
    return 11;
  char basename_4095[4096];
  for (size_t index = 0; index < sizeof(basename_4095) - 1; ++index)
    basename_4095[index] = 'x';
  basename_4095[sizeof(basename_4095) - 1] = '\0';
  if (basename(basename_4095) == NULL || !Equal(basename(basename_4095), basename_4095))
    return 12;
  char basename_4096[4097];
  for (size_t index = 0; index < sizeof(basename_4096) - 1; ++index)
    basename_4096[index] = 'x';
  basename_4096[sizeof(basename_4096) - 1] = '\0';
  errno = 0;
  if (basename(basename_4096) != NULL || errno != ENAMETOOLONG) return 13;
  unsigned char entropy[32] = {0};
  errno = 780;
  if (getentropy(entropy, sizeof(entropy)) != 0 || errno != 780) return 14;
  int any_entropy = 0;
  for (size_t index = 0; index < sizeof(entropy); ++index)
    any_entropy |= entropy[index] != 0;
  if (!any_entropy) return 15;
  unsigned char oversized[257] = {0};
  errno = 0;
  if (getentropy(oversized, sizeof(oversized)) != -1 || errno != EIO) return 16;
  errno = 0;
  void* null_buffer = NULL;
  if (getentropy(null_buffer, 1) != -1 || errno != EFAULT) return 17;
  return 42;
}

__attribute__((visibility("default"))) int
bionic_process_fixture_signal_legacy(void) {
  gSignalSeen = 0;
  void (*old_handler)(int) = signal(SIGUSR1, LegacySignalHandler);
  if (old_handler == SIG_ERR) return 18;
  if (raise(SIGUSR1) != 0 || gSignalSeen != 1) return 19;
  if (signal(SIGUSR1, old_handler) == SIG_ERR) return 20;
  return 42;
}

__attribute__((visibility("default"))) int
bionic_process_fixture_sigaction_query(void) {
  struct sigaction action = {0};
  struct sigaction observed = {0};
  struct sigaction old_action = {0};
  gSignalSeen = 0;
  action.sa_handler = SigactionHandler;
  if (sigaction(SIGUSR2, &action, &old_action) != 0) return 21;
  if (sigaction(SIGUSR2, NULL, &observed) != 0 ||
      observed.sa_handler != SigactionHandler || observed.sa_flags != 0)
    return 22;
  if (raise(SIGUSR2) != 0 || gSignalSeen != 1) return 23;
  action.sa_handler = SIG_IGN;
  if (sigaction(SIGUSR2, &action, NULL) != 0 ||
      sigaction(SIGUSR2, NULL, &observed) != 0 ||
      observed.sa_handler != SIG_IGN || observed.sa_flags != 0)
    return 25;
  action.sa_handler = SIG_DFL;
  if (sigaction(SIGUSR2, &action, NULL) != 0 ||
      sigaction(SIGUSR2, NULL, &observed) != 0 ||
      observed.sa_handler != SIG_DFL || observed.sa_flags != 0)
    return 26;
  if (sigaction(SIGUSR2, &old_action, NULL) != 0) return 24;
  return 42;
}

__attribute__((visibility("default"))) int
bionic_process_fixture_sigaction_context_recovery(void) {
  struct sigaction action = {0};
  struct sigaction old_action = {0};
  gSignalSeen = 0;
  action.sa_sigaction = RecoverFaultHandler;
  action.sa_flags = SA_SIGINFO;
  if (sigaction(SIGSEGV, &action, &old_action) != 0) return 27;
  __asm__ volatile("mov x9, xzr\n\tstr xzr, [x9]" ::: "x9", "memory");
  if (sigaction(SIGSEGV, &old_action, NULL) != 0) return 28;
  if (gSignalSeen != 1) return 29;
  return 42;
}

__attribute__((visibility("default"))) int
bionic_process_fixture_concurrent(void) {
  char value[PROP_VALUE_MAX];
  if (getenv("ANDROID_ROOT") != gEnvironment ||
      __system_property_get("ro.product.cpu.abi", value) != 9 ||
      !Equal(value, "arm64-v8a") || getauxval(AT_RANDOM) != (unsigned long)gRandom ||
      getauxval(AT_PAGESZ) != 16384) return 10;
  return 42;
}

__attribute__((visibility("default"))) int
bionic_process_fixture_verify_pointers(void) {
  if (!Equal(gEnvironment, "/system") || gRandom[0] != 0x10 ||
      gRandom[15] != 0x0f) return 11;
  return 42;
}

__attribute__((visibility("default"))) int
bionic_process_fixture_after_teardown(void) {
  char value[PROP_VALUE_MAX] = {'Q'};
  errno = 0;
  if (getenv("ANDROID_ROOT") != NULL || errno != EIO) return 12;
  errno = 0;
  if (__system_property_get("ro.build.version.sdk", value) != 0 ||
      value[0] != '\0' || errno != EIO) return 13;
  errno = 0;
  if (getauxval(AT_PAGESZ) != 0 || errno != EIO) return 14;
  return 42;
}
