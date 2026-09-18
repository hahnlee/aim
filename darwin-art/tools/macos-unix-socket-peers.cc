#include <libproc.h>
#include <sys/proc_info.h>

#include <algorithm>
#include <charconv>
#include <cerrno>
#include <cstring>
#include <iostream>
#include <limits>
#include <string_view>
#include <vector>

namespace {
enum class Result { kOk, kMissing, kDenied, kFailure };
constexpr int kFailure = 1;
constexpr int kMissing = 2;
constexpr int kDenied = 3;
constexpr int kUsage = 64;

Result Report(pid_t pid, std::string_view operation, int error, int bytes = -1) {
  Result result = Result::kFailure;
  const char* kind = "queryFailed";
  if (error == ESRCH) {
    result = Result::kMissing;
    kind = "PIDmissing";
  } else if (error == EACCES || error == EPERM) {
    result = Result::kDenied;
    kind = "accessDenied";
  }
  std::cerr << "pid=" << pid << "\terror=" << kind
            << "\toperation=" << operation;
  if (bytes >= 0) {
    std::cerr << "\tbytes=" << bytes;
  } else {
    std::cerr << "\terrno=" << error << " (" << std::strerror(error) << ")";
  }
  std::cerr << '\n';
  return result;
}

bool ParsePID(const char* text, pid_t* pid) {
  if (text == nullptr || *text == '\0') {
    return false;
  }
  const char* end = text + std::strlen(text);
  unsigned long long value = 0;
  const auto parsed = std::from_chars(text, end, value, 10);
  if (parsed.ec != std::errc() || parsed.ptr != end || value == 0 ||
      value > static_cast<unsigned long long>(std::numeric_limits<pid_t>::max())) {
    return false;
  }
  *pid = static_cast<pid_t>(value);
  return true;
}

Result ReadFDs(pid_t pid, std::vector<proc_fdinfo>* descriptors) {
  constexpr size_t entryBytes = sizeof(proc_fdinfo);
  constexpr size_t maxBytes =
      (static_cast<size_t>(std::numeric_limits<int>::max()) / entryBytes) *
      entryBytes;
  errno = 0;
  const int initial = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, nullptr, 0);
  if (initial < 0 || (initial == 0 && errno != 0)) {
    return Report(pid, "PROC_PIDLISTFDS.size", errno);
  }
  if (initial == 0) {
    descriptors->clear();
    return Result::kOk;
  }
  const size_t initialBytes = static_cast<size_t>(initial);
  if (initialBytes % entryBytes != 0 || initialBytes >= maxBytes) {
    return Report(pid, "PROC_PIDLISTFDS.size", EOVERFLOW, initial);
  }

  size_t requestBytes = initialBytes + entryBytes;
  std::vector<proc_fdinfo> buffer;
  for (;;) {
    try {
      buffer.resize(requestBytes / entryBytes);
    } catch (const std::bad_alloc&) {
      return Report(pid, "PROC_PIDLISTFDS.buffer", ENOMEM);
    }
    errno = 0;
    const int returned = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, buffer.data(),
                                      static_cast<int>(requestBytes));
    if (returned < 0 || (returned == 0 && errno != 0)) {
      return Report(pid, "PROC_PIDLISTFDS", errno);
    }
    if (returned == 0) {
      descriptors->clear();
      return Result::kOk;
    }
    const size_t returnedBytes = static_cast<size_t>(returned);
    if (returnedBytes > requestBytes || returnedBytes % entryBytes != 0) {
      return Report(pid, "PROC_PIDLISTFDS", EPROTO, returned);
    }
    if (returnedBytes == requestBytes) {
      if (requestBytes == maxBytes) {
        return Report(pid, "PROC_PIDLISTFDS.capacity", EOVERFLOW, returned);
      }
      requestBytes = std::min(maxBytes, std::max(requestBytes + entryBytes,
                                                  requestBytes * 2));
      continue;
    }
    buffer.resize(returnedBytes / entryBytes);
    descriptors->swap(buffer);
    return Result::kOk;
  }
}

Result ReadSocket(pid_t pid, const proc_fdinfo& descriptor,
                  socket_fdinfo* info) {
  errno = 0;
  const int returned = proc_pidfdinfo(pid, descriptor.proc_fd,
                                      PROC_PIDFDSOCKETINFO, info, sizeof(*info));
  if (returned <= 0) {
    return Report(pid, "PROC_PIDFDSOCKETINFO", errno != 0 ? errno : EIO);
  }
  if (returned != static_cast<int>(sizeof(*info))) {
    return Report(pid, "PROC_PIDFDSOCKETINFO", EPROTO, returned);
  }
  return Result::kOk;
}

void Record(Result result, int* status) {
  if (result == Result::kMissing) {
    *status = std::max(*status, kMissing);
  } else if (result == Result::kDenied) {
    *status = std::max(*status, kDenied);
  } else if (result == Result::kFailure) {
    *status = std::max(*status, kFailure);
  }
}
}  // namespace

int main(int argc, char** argv) {
  if (argc < 2 || argc > 9) {
    std::cerr << "usage: macos-unix-socket-peers PID [PID ...]  (1..8 PIDs)\n";
    return kUsage;
  }
  std::vector<pid_t> pids;
  for (int i = 1; i < argc; ++i) {
    pid_t pid = 0;
    if (!ParsePID(argv[i], &pid)) {
      std::cerr << "invalid positive PID: " << argv[i] << '\n';
      return kUsage;
    }
    pids.push_back(pid);
  }

  std::cout << "pid\tfd\tsoi_so\tsoi_pcb\tunsi_conn_so\tunsi_conn_pcb\ttype\tstate\n";
  int status = 0;
  for (const pid_t pid : pids) {
    std::vector<proc_fdinfo> descriptors;
    const Result list = ReadFDs(pid, &descriptors);
    if (list != Result::kOk) {
      Record(list, &status);
      continue;
    }
    for (const proc_fdinfo& descriptor : descriptors) {
      if (descriptor.proc_fdtype != PROX_FDTYPE_SOCKET) {
        continue;
      }
      socket_fdinfo info{};
      const Result socket = ReadSocket(pid, descriptor, &info);
      if (socket != Result::kOk) {
        Record(socket, &status);
        continue;
      }
      if (info.psi.soi_kind != SOCKINFO_UN) {
        continue;
      }
      const un_sockinfo& unixInfo = info.psi.soi_proto.pri_un;
      std::cout << pid << '\t' << descriptor.proc_fd << '\t' << info.psi.soi_so
                << '\t' << info.psi.soi_pcb << '\t' << unixInfo.unsi_conn_so
                << '\t' << unixInfo.unsi_conn_pcb << '\t' << info.psi.soi_type
                << '\t' << info.psi.soi_state << '\n';
    }
  }
  return status;
}
