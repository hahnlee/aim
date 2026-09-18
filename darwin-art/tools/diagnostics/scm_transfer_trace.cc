// Standalone, opt-in macOS diagnostic interposer.  It is intentionally not
// linked into Darwin ART or any production target.
#include <arpa/inet.h>
#include <errno.h>
#include <libproc.h>
#include <pthread.h>
#include <sys/proc_info.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstdarg>
#include <cstring>

namespace {

constexpr size_t kMaxRights = 16;
constexpr size_t kMaxControl = 64 * 1024;
constexpr size_t kMaxLine = 16 * 1024;
constexpr unsigned kMaxRecords = 256;

// Darwin arm64 SDK ABI, independently checked by the repository's
// bionic-socket-facade audit: msghdr=48, cmsghdr=12, 4-byte cmsg alignment.
constexpr int kDarwinSolSocket = 0xffff;
constexpr int kScmRights = 1;
constexpr size_t kCmsgHeader = 12;
constexpr size_t kCmsgAlign = 4;

thread_local bool g_in_trace = false;
std::atomic<unsigned> g_records{0};
std::atomic<bool> g_overflow_announced{false};
// Image/memfd traffic must not exhaust the separate socket-transfer budget.
std::atomic<unsigned> g_socket_records{0};
std::atomic<bool> g_socket_overflow_announced{false};
std::atomic<uint64_t> g_calls{0};
std::atomic<uint64_t> g_rights_calls{0};
std::atomic<uint64_t> g_decode_errors{0};
std::atomic<bool> g_seen_send{false};
std::atomic<bool> g_seen_receive{false};

struct Identity {
  const char *kind = "queryFailed";
  uint64_t so = 0;
  uint64_t pcb = 0;
  uint64_t conn_so = 0;
  uint64_t conn_pcb = 0;
  int type = 0;
  int state = 0;
  int error = 0;
};

struct Rights {
  int fd[kMaxRights]{};
  size_t count = 0;
  bool ctrunc = false;
  const char *error = nullptr;
};

bool TraceEnabled() {
  const char *value = std::getenv("DARWIN_ART_TRACE_SCM");
  return value != nullptr && value[0] != '\0' && std::strcmp(value, "0") != 0;
}

uint64_t ThreadId() {
  uint64_t id = 0;
  (void)pthread_threadid_np(nullptr, &id);
  return id;
}

uint64_t WallNs() {
  timespec now{};
  clock_gettime(CLOCK_REALTIME, &now);
  return static_cast<uint64_t>(now.tv_sec) * 1000000000ULL +
         static_cast<uint64_t>(now.tv_nsec);
}

class Line {
 public:
  bool Add(const char *format, ...) {
    if (!ok_) return false;
    va_list args;
    va_start(args, format);
    const int written = std::vsnprintf(buffer_ + length_,
                                       sizeof(buffer_) - length_, format, args);
    va_end(args);
    if (written < 0 || static_cast<size_t>(written) >= sizeof(buffer_) - length_) {
      ok_ = false;
      return false;
    }
    length_ += static_cast<size_t>(written);
    return true;
  }

  const char *data() const { return buffer_; }
  size_t size() const { return length_; }
  bool ok() const { return ok_; }

 private:
  char buffer_[kMaxLine]{};
  size_t length_ = 0;
  bool ok_ = true;
};

void AppendU64String(Line *line, uint64_t value) {
  line->Add("\"%llu\"", static_cast<unsigned long long>(value));
}

void EmitLoadedMarker() {
  if (!TraceEnabled()) return;
  Line line;
  line.Add("{\"event\":\"scm_trace_loaded\",\"pid\":%d,\"tid\":%llu,"
           "\"wall_ns\":", getpid(),
           static_cast<unsigned long long>(ThreadId()));
  AppendU64String(&line, WallNs());
  line.Add("}\n");
  (void)write(STDERR_FILENO, line.data(), line.size());
}

__attribute__((constructor)) void ScmTraceLoaded() {
  const int saved_errno = errno;
  EmitLoadedMarker();
  errno = saved_errno;
}

Identity InspectFd(int fd) {
  Identity identity;
  socket_fdinfo info{};
  errno = 0;
  const int result = proc_pidfdinfo(getpid(), fd, PROC_PIDFDSOCKETINFO,
                                    &info, sizeof(info));
  if (result == static_cast<int>(sizeof(info))) {
    identity.kind = info.psi.soi_kind == SOCKINFO_UN ? "unixSocket" :
                                                       "otherSocket";
    identity.so = static_cast<uint64_t>(info.psi.soi_so);
    identity.pcb = static_cast<uint64_t>(info.psi.soi_pcb);
    if (info.psi.soi_kind == SOCKINFO_UN) {
      const un_sockinfo &unix_info = info.psi.soi_proto.pri_un;
      identity.conn_so = static_cast<uint64_t>(unix_info.unsi_conn_so);
      identity.conn_pcb = static_cast<uint64_t>(unix_info.unsi_conn_pcb);
    }
    identity.type = info.psi.soi_type;
    identity.state = info.psi.soi_state;
    return identity;
  }
  identity.error = errno;
  if (errno == EACCES || errno == EPERM) {
    identity.kind = "denied";
  } else if (errno == EBADF || errno == ESRCH) {
    identity.kind = "missing";
  } else if (errno == EINVAL) {
    // proc_pidfdinfo reports EINVAL for a live descriptor that is not a
    // socket; this is distinct from EBADF/ESRCH above.
    identity.kind = "nonsocket";
  }
  return identity;
}

void AppendIdentity(Line *line, int fd, const Identity &identity) {
  line->Add("{\"fd\":%d,\"kind\":\"%s\",\"so\":",
            fd, identity.kind);
  AppendU64String(line, identity.so);
  line->Add(",\"pcb\":");
  AppendU64String(line, identity.pcb);
  line->Add(",\"conn_so\":");
  AppendU64String(line, identity.conn_so);
  line->Add(",\"conn_pcb\":");
  AppendU64String(line, identity.conn_pcb);
  line->Add(",\"type\":%d,\"state\":%d,\"error\":%d}",
            identity.type, identity.state, identity.error);
}

bool DecodeRights(const struct msghdr *message, Rights *rights) {
  if (message == nullptr) {
    rights->error = "null_msghdr";
    return false;
  }
  rights->ctrunc = (message->msg_flags & MSG_CTRUNC) != 0;
  if (message->msg_controllen > kMaxControl) {
    rights->error = "control_too_large";
    return false;
  }
  const auto *bytes = static_cast<const uint8_t *>(message->msg_control);
  if (message->msg_controllen != 0 && bytes == nullptr) {
    rights->error = "null_control";
    return false;
  }
  size_t offset = 0;
  while (offset < message->msg_controllen) {
    if (message->msg_controllen - offset < kCmsgHeader) {
      rights->error = "truncated_cmsghdr";
      return false;
    }
    uint32_t length = 0;
    int32_t level = 0;
    int32_t type = 0;
    std::memcpy(&length, bytes + offset, sizeof(length));
    std::memcpy(&level, bytes + offset + 4, sizeof(level));
    std::memcpy(&type, bytes + offset + 8, sizeof(type));
    if (length < kCmsgHeader || length > message->msg_controllen - offset) {
      rights->error = "malformed_cmsghdr";
      return false;
    }
    if (level == kDarwinSolSocket && type == kScmRights) {
      const size_t payload = length - kCmsgHeader;
      if (payload % sizeof(int) != 0) {
        rights->error = "truncated_rights";
        return false;
      }
      const size_t count = payload / sizeof(int);
      if (count > kMaxRights || rights->count + count > kMaxRights) {
        rights->error = "rights_bound";
        return false;
      }
      for (size_t index = 0; index < count; ++index) {
        int fd = -1;
        std::memcpy(&fd, bytes + offset + kCmsgHeader + index * sizeof(int),
                    sizeof(fd));
        rights->fd[rights->count++] = fd;
      }
    }
    const size_t next = (offset + length + kCmsgAlign - 1) &
                        ~(kCmsgAlign - 1);
    if (next <= offset || next > message->msg_controllen) {
      rights->error = "truncated_cmsg_alignment";
      return false;
    }
    offset = next;
  }
  rights->ctrunc = (message->msg_flags & MSG_CTRUNC) != 0;
  return true;
}

void EmitEvent(const char *event, int carrier, const Rights &rights,
               ssize_t result, bool has_result, const char *phase,
               uint64_t pre_wall_ns, uintptr_t caller, int operation_flags) {
  Identity identities[1 + kMaxRights];
  identities[0] = InspectFd(carrier);
  bool socket_rights = false;
  for (size_t index = 0; index < rights.count; ++index) {
    identities[index + 1] = InspectFd(rights.fd[index]);
    if (std::strcmp(identities[index + 1].kind, "unixSocket") == 0 ||
        std::strcmp(identities[index + 1].kind, "otherSocket") == 0)
      socket_rights = true;
  }
  auto &records = socket_rights ? g_socket_records : g_records;
  auto &announced = socket_rights ? g_socket_overflow_announced :
                                  g_overflow_announced;
  const unsigned record = records.fetch_add(1, std::memory_order_relaxed);
  if (record >= kMaxRecords) {
    if (record == kMaxRecords &&
        !announced.exchange(true, std::memory_order_relaxed)) {
      Line overflow;
      overflow.Add("{\"event\":\"scm_trace_overflow\",\"pid\":%d,"
                   "\"record_limit\":%u,\"category\":\"%s\"}\n",
                   getpid(), kMaxRecords,
                   socket_rights ? "socket_rights" : "other");
      (void)write(STDERR_FILENO, overflow.data(), overflow.size());
    }
    return;
  }

  Line line;
  line.Add("{\"event\":\"%s\",\"pid\":%d,\"tid\":%llu,"
           "\"wall_ns\":", event, getpid(),
           static_cast<unsigned long long>(ThreadId()));
  AppendU64String(&line, WallNs());
  line.Add(",\"pre_wall_ns\":");
  AppendU64String(&line, pre_wall_ns);
  line.Add(",\"phase\":\"%s\",\"category\":\"%s\","
           "\"carrier_fd\":%d,\"rights\":[",
           phase, socket_rights ? "socket_rights" : "other", carrier);
  for (size_t index = 0; index < rights.count; ++index) {
    if (index) line.Add(",");
    line.Add("%d", rights.fd[index]);
  }
  line.Add("],\"ctrunc\":%s", rights.ctrunc ? "true" : "false");
  line.Add(",\"operation_flags\":%d", operation_flags);
  line.Add(",\"caller\":\"%llx\",\"calls\":%llu,\"rights_calls\":%llu,"
           "\"decode_errors\":%llu", static_cast<unsigned long long>(caller),
           static_cast<unsigned long long>(g_calls.load()),
           static_cast<unsigned long long>(g_rights_calls.load()),
           static_cast<unsigned long long>(g_decode_errors.load()));
  if (has_result) line.Add(",\"result\":%lld",
                           static_cast<long long>(result));
  if (rights.error != nullptr)
    line.Add(",\"decode_error\":\"%s\"", rights.error);
  line.Add(",\"identities\":[");
  const size_t total = 1 + rights.count;
  for (size_t index = 0; index < total; ++index) {
    if (index) line.Add(",");
    const int fd = index == 0 ? carrier : rights.fd[index - 1];
    AppendIdentity(&line, fd, identities[index]);
  }
  line.Add("]}\n");
  if (line.ok()) (void)write(STDERR_FILENO, line.data(), line.size());
}

ssize_t CallSendmsg(int fd, const struct msghdr *message, int flags) {
  // dyld does not interpose this replacement image's own original binding.
  return ::sendmsg(fd, message, flags);
}

ssize_t CallRecvmsg(int fd, struct msghdr *message, int flags) {
  return ::recvmsg(fd, message, flags);
}

}  // namespace

extern "C" ssize_t darwin_art_trace_sendmsg(int fd,
                                             const struct msghdr *message,
                                             int flags) {
  const int incoming_errno = errno;
  const bool enabled = TraceEnabled();
  errno = incoming_errno;
  if (g_in_trace || !enabled) return CallSendmsg(fd, message, flags);
  g_in_trace = true;
  g_calls.fetch_add(1, std::memory_order_relaxed);
  const bool first = !g_seen_send.exchange(true, std::memory_order_relaxed);
  const auto caller = reinterpret_cast<uintptr_t>(__builtin_return_address(0)) &
                      ((uintptr_t{1} << 48) - 1);
  const uint64_t pre_wall_ns = WallNs();
  errno = incoming_errno;
  const ssize_t result = CallSendmsg(fd, message, flags);
  const int saved_errno = errno;
  if (result >= 0) {
    Rights rights;
    if (!DecodeRights(message, &rights)) g_decode_errors.fetch_add(1);
    if (rights.count) g_rights_calls.fetch_add(1);
    if (first || rights.count || rights.error || rights.ctrunc)
      EmitEvent("sendmsg", fd, rights, result, true,
                "post_success_pre_caller_cleanup", pre_wall_ns, caller, flags);
  }
  errno = saved_errno;
  g_in_trace = false;
  return result;
}

extern "C" ssize_t darwin_art_trace_recvmsg(int fd, struct msghdr *message,
                                             int flags) {
  const int incoming_errno = errno;
  const bool enabled = TraceEnabled();
  errno = incoming_errno;
  if (g_in_trace || !enabled) return CallRecvmsg(fd, message, flags);
  g_in_trace = true;
  g_calls.fetch_add(1, std::memory_order_relaxed);
  const bool first = !g_seen_receive.exchange(true, std::memory_order_relaxed);
  const auto caller = reinterpret_cast<uintptr_t>(__builtin_return_address(0)) &
                      ((uintptr_t{1} << 48) - 1);
  const uint64_t pre_wall_ns = WallNs();
  errno = incoming_errno;
  const ssize_t result = CallRecvmsg(fd, message, flags);
  const int saved_errno = errno;
  if (result >= 0) {
    Rights rights;
    // Darwin ancillary peek returns zero placeholders, not externalized FDs.
    // An operation audit must not attribute stdin's identity to those integers.
    if (!(flags & MSG_PEEK) && !DecodeRights(message, &rights))
      g_decode_errors.fetch_add(1);
    if (rights.count) g_rights_calls.fetch_add(1);
    if (first || rights.count || rights.error || rights.ctrunc ||
        (flags & MSG_PEEK))
      EmitEvent("recvmsg", fd, rights, result, true, "post_success", pre_wall_ns,
                caller, flags);
  } else if (flags & MSG_PEEK) {
    // Failed peek still proves the operation was requested. Never inspect the
    // caller's message on a failed syscall, including EFAULT.
    Rights rights;
    EmitEvent("recvmsg", fd, rights, result, true, "post_failure", pre_wall_ns,
              caller, flags);
  }
  errno = saved_errno;
  g_in_trace = false;
  return result;
}

namespace {
template <typename Operation>
ssize_t AuditBytePeek(const char *event, int fd, int flags, uintptr_t caller,
                     Operation operation) {
  const int incoming_errno = errno;
  const bool enabled = TraceEnabled();
  errno = incoming_errno;
  if (g_in_trace || !enabled || !(flags & MSG_PEEK)) return operation();
  g_in_trace = true;
  g_calls.fetch_add(1, std::memory_order_relaxed);
  const uint64_t pre_wall_ns = WallNs();
  errno = incoming_errno;
  const ssize_t result = operation();
  const int saved_errno = errno;
  Rights rights;
  EmitEvent(event, fd, rights, result, true,
            result < 0 ? "post_failure" : "post_success", pre_wall_ns,
            caller, flags);
  errno = saved_errno;
  g_in_trace = false;
  return result;
}
}  // namespace

extern "C" ssize_t darwin_art_trace_recv(int fd, void *bytes, size_t count,
                                          int flags) {
  const auto caller = reinterpret_cast<uintptr_t>(__builtin_return_address(0)) &
                      ((uintptr_t{1} << 48) - 1);
  return AuditBytePeek("recv", fd, flags, caller,
                       [&] { return ::recv(fd, bytes, count, flags); });
}

extern "C" ssize_t darwin_art_trace_recvfrom(
    int fd, void *bytes, size_t count, int flags, sockaddr *address,
    socklen_t *address_length) {
  const auto caller = reinterpret_cast<uintptr_t>(__builtin_return_address(0)) &
                      ((uintptr_t{1} << 48) - 1);
  return AuditBytePeek("recvfrom", fd, flags, caller, [&] {
    return ::recvfrom(fd, bytes, count, flags, address, address_length);
  });
}

struct InterposeEntry {
  const void *replacement;
  const void *replacee;
};

__attribute__((used, section("__DATA,__interpose")))
static const InterposeEntry kInterpose[] = {
    {reinterpret_cast<const void *>(&darwin_art_trace_sendmsg),
     reinterpret_cast<const void *>(&sendmsg)},
    {reinterpret_cast<const void *>(&darwin_art_trace_recvmsg),
     reinterpret_cast<const void *>(&recvmsg)},
    {reinterpret_cast<const void *>(&darwin_art_trace_recv),
     reinterpret_cast<const void *>(&recv)},
    {reinterpret_cast<const void *>(&darwin_art_trace_recvfrom),
     reinterpret_cast<const void *>(&recvfrom)},
};
