#include "aim_bionic_syslog.h"
#include "aim_bionic_errno.h"
#include "aim_elf_loader.h"

#include <android/log.h>
#include <errno.h>
#include <unistd.h>

#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <mutex>
#include <string>
#include <vector>

namespace {

struct Event {
  int priority;
  std::string tag;
  std::string message;
};

std::mutex g_lock;
std::vector<Event> g_events;

void Capture(const __android_log_message* message) {
  if (message == nullptr ||
      message->struct_size < sizeof(__android_log_message)) {
    std::abort();
  }
  std::lock_guard<std::mutex> lock(g_lock);
  g_events.push_back({message->priority,
                      message->tag == nullptr ? "" : message->tag,
                      message->message == nullptr ? "" : message->message});
}

AimElfResolveStatus Resolve(void*,
                                  const AimElfSymbolRequest* request,
                                  uintptr_t* output,
                                  AimElfErrorBuffer*) {
  if (request == nullptr || output == nullptr || request->symbol == nullptr ||
      request->version_soname == nullptr || request->version_name == nullptr) {
    return AIM_ELF_RESOLVE_ERROR;
  }
  auto function = aim_bionic_syslog_resolve(
      request->version_soname, request->symbol, request->version_name);
  if (function == nullptr) return AIM_ELF_RESOLVE_NOT_FOUND;
  *output = reinterpret_cast<uintptr_t>(function);
  return AIM_ELF_RESOLVE_FOUND;
}

template <typename Function>
Function Lookup(AimElfHandle* image, const char* name) {
  uintptr_t address = 0;
  char message[256]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  if (aim_elf_lookup(image, name, &address, &error) !=
          AIM_ELF_OK ||
      address == 0) {
    std::fprintf(stderr, "lookup %s: %s\n", name, message);
    std::abort();
  }
  return reinterpret_cast<Function>(address);
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) return 10;
  std::ifstream input(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), {});
  if (bytes.empty() || input.bad()) return 11;

  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve, nullptr};
  AimElfHandle* image = nullptr;
  char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  if (aim_elf_load_bytes(bytes.data(), bytes.size(), &options, &image,
                                &error) != AIM_ELF_OK ||
      aim_elf_run_initializers(image, &error) != AIM_ELF_OK) {
    std::fprintf(stderr, "load: %s\n", message);
    return 12;
  }

  using Run = void (*)();
  Run run = Lookup<Run>(image, "SyslogFixtureRun");
  Run after_close = Lookup<Run>(image, "SyslogFixtureAfterClose");
  Run unsupported = Lookup<Run>(image, "SyslogFixtureUnsupported");
  __android_log_set_logger(Capture);
  aim_bionic_errno_store(0);
  after_close();
  if (aim_bionic_errno_load() != 95 || !g_events.empty()) return 13;
  char guest_program_tag[] = "GuestProgram";
  if (aim_bionic_syslog_activate(guest_program_tag) != 0 ||
      aim_bionic_syslog_activate("SecondProgram") != -1) {
    return 14;
  }
  guest_program_tag[0] = 'X';

  int pipe_fds[2];
  if (pipe(pipe_fds) != 0) return 15;
  const int saved_stderr = dup(STDERR_FILENO);
  if (saved_stderr < 0 || dup2(pipe_fds[1], STDERR_FILENO) < 0) return 16;
  close(pipe_fds[1]);
  errno = EDOM;
  run();
  const int observed_errno = errno;
  const int restore_result = dup2(saved_stderr, STDERR_FILENO);
  if (observed_errno != EDOM || restore_result < 0) {
    std::fprintf(stderr, "errno preservation: got=%d restore=%d\n",
                 observed_errno, restore_result);
    return 17;
  }
  close(saved_stderr);
  char perror_output[256]{};
  const ssize_t perror_size = read(pipe_fds[0], perror_output,
                                   sizeof(perror_output) - 1);
  close(pipe_fds[0]);
  if (perror_size < 0) {
    std::perror("read LOG_PERROR pipe");
    return 18;
  }

  after_close();
  aim_bionic_errno_store(0);
  unsupported();
  if (aim_bionic_errno_load() != 95) return 19;
  __android_log_set_logger(__android_log_stderr_logger);
  {
    std::lock_guard<std::mutex> lock(g_lock);
    if (g_events.size() != 2 || g_events[0].priority != ANDROID_LOG_WARN ||
        g_events[0].tag != "GuestTag" ||
        g_events[0].message != "value=17 name=android fp=2.5" ||
        g_events[1].priority != ANDROID_LOG_INFO ||
        g_events[1].tag != "GuestProgram" ||
        g_events[1].message != "after-close") {
      for (const Event& event : g_events) {
        std::fprintf(stderr, "event priority=%d tag=%s message=%s\n",
                     event.priority, event.tag.c_str(), event.message.c_str());
      }
      return 20;
    }
  }
  if (std::string(perror_output, static_cast<size_t>(perror_size)) !=
      "GuestTag: value=17 name=android fp=2.5\n") {
    std::fprintf(stderr, "PERROR mismatch: <%.*s>\n",
                 static_cast<int>(perror_size), perror_output);
    return 21;
  }
  if (aim_elf_unload(&image, &error) != AIM_ELF_OK ||
      image != nullptr) {
    return 22;
  }
  std::fprintf(stderr,
               "bionic-syslog-facade: ELF PASS AAPCS64-varargs liblog=2 "
               "priority+facility+state+PERROR+errno\n");
  return 0;
}
