// art::tools::EnsureNoProcessInDir (libarttools) for Darwin. AOSP finds the
// processes whose /proc/<pid>/exe lies under `dir`, waits for them through
// pidfds and optionally kills them. Darwin has neither procfs nor pidfds:
// processes are enumerated with libproc, their executable paths come from
// proc_pidpath and exits are observed with kqueue EVFILT_PROC. Executable
// paths are host paths, so `dir` is compared through its host backing.
// fstab/fstab.h (through tools.h) uses bionic's off64_t.
#include "android-base/off64_t.h"
#include "tools/tools.h"

#include <libproc.h>
#include <signal.h>
#include <sys/event.h>
#include <sys/time.h>
#include <unistd.h>

#include <chrono>
#include <cstdint>
#include <map>
#include <string>
#include <string_view>
#include <vector>

#include "android-base/logging.h"
#include "android-base/result.h"
#include "android-base/unique_fd.h"

extern "C" intptr_t darwin_art_bionic_fs_resolve_private_host_path(
    const char* path, char* output, size_t capacity);

namespace art {
namespace tools {
namespace {

using android::base::Result;

// A guest path with host backing resolves to it; any other guest directory
// holds no host executable.
std::string HostDirectory(const std::string& dir) {
  const intptr_t length = darwin_art_bionic_fs_resolve_private_host_path(dir.c_str(), nullptr, 0);
  if (length <= 0) return dir;
  std::string host(static_cast<size_t>(length) + 1, '\0');
  if (darwin_art_bionic_fs_resolve_private_host_path(dir.c_str(), host.data(), host.size()) !=
      length) {
    return dir;
  }
  host.resize(static_cast<size_t>(length));
  return host;
}

std::vector<pid_t> AllPids() {
  std::vector<pid_t> pids(static_cast<size_t>(std::max(proc_listallpids(nullptr, 0), 0)) + 64);
  const int count = proc_listallpids(pids.data(), static_cast<int>(pids.size() * sizeof(pid_t)));
  pids.resize(count > 0 ? static_cast<size_t>(count) : 0);
  return pids;
}

uint64_t MilliTime() {
  return std::chrono::duration_cast<std::chrono::milliseconds>(
             std::chrono::steady_clock::now().time_since_epoch())
      .count();
}

}  // namespace

Result<void> EnsureNoProcessInDir(const std::string& dir, uint32_t timeout_ms, bool try_kill) {
  const std::string host_dir = HostDirectory(dir);
  android::base::unique_fd queue(kqueue());
  if (queue < 0) return ErrnoErrorf("Failed to create kqueue");
  std::map<pid_t, std::string> running_processes;
  for (pid_t pid : AllPids()) {
    if (pid <= 0) continue;
    char exe[PROC_PIDPATHINFO_MAXSIZE];
    // The caller may not have access to all processes.
    if (proc_pidpath(pid, exe, sizeof(exe)) <= 0) continue;
    if (!PathStartsWith(exe, host_dir)) continue;
    struct kevent change;
    EV_SET(&change, pid, EVFILT_PROC, EV_ADD | EV_ONESHOT, NOTE_EXIT, 0, nullptr);
    if (kevent(queue.get(), &change, 1, nullptr, 0, nullptr) != 0) {
      if (errno == ESRCH) continue;  // The process has gone now.
      return ErrnoErrorf("Failed to watch pid {}", pid);
    }
    char name[2 * MAXCOMLEN + 1] = {};
    proc_name(pid, name, sizeof(name));
    LOG(INFO) << "Process '" << name << "' (pid: " << pid
              << ") is still running. Waiting for it to exit";
    running_processes[pid] = name;
  }

  auto wait_for_processes = [&]() -> Result<void> {
    const uint64_t start_time_ms = MilliTime();
    uint64_t remaining_timeout_ms = timeout_ms;
    while (!running_processes.empty() && remaining_timeout_ms > 0) {
      struct kevent event;
      const struct timespec timeout = {
          static_cast<time_t>(remaining_timeout_ms / 1000),
          static_cast<long>((remaining_timeout_ms % 1000) * 1000000)};
      const int ready = kevent(queue.get(), nullptr, 0, &event, 1, &timeout);
      if (ready < 0 && errno != EINTR) return ErrnoErrorf("Failed to wait for processes");
      if (ready == 0) break;  // Timeout.
      const uint64_t elapsed_time_ms = MilliTime() - start_time_ms;
      if (ready > 0) {
        const auto process = running_processes.find(static_cast<pid_t>(event.ident));
        if (process != running_processes.end()) {
          LOG(INFO) << "Process '" << process->second << "' (pid: " << process->first
                    << ") exited in " << elapsed_time_ms << "ms";
          running_processes.erase(process);
        }
      }
      remaining_timeout_ms = elapsed_time_ms >= timeout_ms ? 0 : timeout_ms - elapsed_time_ms;
    }
    return {};
  };

  if (auto result = wait_for_processes(); !result.ok()) return result;
  bool process_killed = false;
  for (const auto& [pid, name] : running_processes) {
    LOG(ERROR) << "Process '" << name << "' (pid: " << pid << ") is still running after "
               << timeout_ms << "ms";
    if (try_kill) {
      LOG(INFO) << "Killing '" << name << "' (pid: " << pid << ")";
      if (kill(pid, SIGKILL) != 0) PLOG(ERROR) << "Failed to kill '" << name << "'";
      process_killed = true;
    }
  }
  if (process_killed) {
    // Wait another round for processes to exit after being killed.
    if (auto result = wait_for_processes(); !result.ok()) return result;
  }
  if (!running_processes.empty()) {
    return Errorf("Some process(es) are still running after {}ms", timeout_ms);
  }
  return {};
}

// libarttools PathStartsWith.
bool PathStartsWith(std::string_view path, std::string_view prefix) {
  CHECK(!prefix.empty() && !path.empty() && prefix[0] == '/' && path[0] == '/')
      << "path=" << path << ", prefix=" << prefix;
  if (prefix.ends_with('/')) prefix.remove_suffix(1);
  return path.starts_with(prefix) &&
         (path.length() == prefix.length() || path[prefix.length()] == '/');
}

}  // namespace tools
}  // namespace art
