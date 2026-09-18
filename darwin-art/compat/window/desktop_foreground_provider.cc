#include "desktop_foreground_provider.h"

#include <ApplicationServices/ApplicationServices.h>
#include <libproc.h>
#include <sys/proc_info.h>
#include <sys/types.h>

namespace darwin_art::window {
namespace {

bool SameBirth(const ProcessIdentity& left,
               const ProcessIdentity& right) noexcept {
  return left.pid == right.pid &&
         left.start_seconds == right.start_seconds &&
         left.start_microseconds == right.start_microseconds;
}

bool CaptureProcessIdentityInternal(int pid, ProcessIdentity* output) noexcept {
  if (pid <= 0 || output == nullptr) return false;

  proc_bsdinfo info{};
  const int bytes = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, sizeof(info));
  if (bytes != static_cast<int>(sizeof(info)) || info.pbi_pid == 0 ||
      static_cast<int>(info.pbi_pid) != pid ||
      info.pbi_start_tvsec == 0 || info.pbi_start_tvusec >= 1000000) {
    return false;
  }

  ProcessIdentity captured;
  captured.pid = info.pbi_pid;
  captured.start_seconds = static_cast<std::uint64_t>(info.pbi_start_tvsec);
  captured.start_microseconds = static_cast<std::uint64_t>(info.pbi_start_tvusec);
  *output = captured;
  return true;
}

bool QueryFrontProcessPid(int* pid) noexcept {
  if (pid == nullptr) return false;
  ProcessSerialNumber serial{};
  pid_t front_pid = 0;

// These Carbon process APIs are deprecated on current macOS, but remain the
// synchronous public front-process authority. Keep the suppression at this
// narrow provider callsite rather than leaking it into callers.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
  const OSStatus front_status = GetFrontProcess(&serial);
  const OSStatus pid_status = front_status == noErr
      ? GetProcessPID(&serial, &front_pid)
      : front_status;
#pragma clang diagnostic pop

  if (pid_status != noErr || front_pid <= 0) return false;
  *pid = static_cast<int>(front_pid);
  return *pid > 0;
}

}  // namespace

bool CaptureProcessIdentity(int pid, ProcessIdentity* output) noexcept {
  return CaptureProcessIdentityInternal(pid, output);
}

bool IsProcessForeground(const ProcessIdentity& identity) noexcept {
  if (identity.pid <= 0 || identity.start_seconds == 0 ||
      identity.start_microseconds >= 1000000) {
    return false;
  }

  ProcessIdentity before;
  if (!CaptureProcessIdentityInternal(identity.pid, &before) ||
      !SameBirth(identity, before)) {
    return false;
  }

  int front_pid = 0;
  if (!QueryFrontProcessPid(&front_pid)) return false;

  ProcessIdentity after;
  if (!CaptureProcessIdentityInternal(identity.pid, &after) ||
      !SameBirth(before, after) || !SameBirth(identity, after)) {
    return false;
  }
  return front_pid == identity.pid;
}

}  // namespace darwin_art::window
