#include "darwin_art_bionic_socket_broker.h"

#include "darwin_art_bionic_binder_fd.h"
#include "darwin_art_bionic_dns.h"
#include "darwin_art_bionic_errno.h"
#include "darwin_art_bionic_fd_broker.h"
#include "darwin_art_bionic_fs.h"
#include "fdsan.h"
#include "fdsan_symbols.h"
#include "sync_fence_broker.h"
#include "unix_endpoints.h"
#include "eventfd_owner.h"
#include "android_scm_exports.h"
#include "fd_inheritance.h"
#include "retained_scm_export.h"
#include "scm_endpoint_lease.h"

namespace native_fd = darwin_art::bionic::fd_inheritance;

#include <arpa/inet.h>
#include <arpa/nameser.h>
#include <errno.h>
#include <fcntl.h>
#include <net/if.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <pthread.h>
#include <resolv.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <climits>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <memory>
#include <mutex>
#include <new>
#include <string>
#include <thread>
#include <unordered_map>
#include <vector>

extern "C" void darwin_art_bionic_errno_store(int32_t android_errno);
extern "C" int darwin_art_bionic_fs_close_core(int fd);
extern "C" int darwin_art_bionic_fs_fcntl_core(int fd, int command,
                                               intptr_t argument);
extern "C" int darwin_art_bionic_fs_dup_host_fd_core(int fd, int *host_fd);
extern "C" int darwin_art_bionic_fs_adopt_host_fd_core(int host_fd);
extern "C" intptr_t darwin_art_bionic_fs_read_core(int fd, void *buffer,
                                                   size_t count);
extern "C" intptr_t darwin_art_bionic_fs_write_core(int fd, const void *buffer,
                                                    size_t count);
extern "C" __attribute__((weak)) int
darwin_art_android_shared_memory_close(int) {
  return 0;
}

// Keep legacy resolver storage in the Android facade, never return Darwin's
// static hostent directly to guest code.
extern "C" struct hostent *
darwin_art_bionic_socket_broker_gethostbyname(const char *name) {
  return darwin_art_bionic_dns_gethostbyname(name);
}

extern "C" struct hostent *
darwin_art_bionic_socket_broker_gethostbyaddr(const void *address,
                                              socklen_t length, int type) {
  (void)address;
  (void)length;
  (void)type;
  return nullptr;
}
extern "C" __attribute__((weak)) int darwin_art_android_shared_memory_dup(int) {
  return -2;
}
extern "C" __attribute__((weak)) int
darwin_art_android_shared_memory_fcntl(int, int, intptr_t, int *) {
  return 0;
}
extern "C" __attribute__((weak)) int
darwin_art_android_shared_memory_get_info(int, size_t *, int *) {
  return 0;
}

namespace {

constexpr int kAndroidAfInet = 2;
constexpr int kAndroidAfInet6 = 10;
constexpr int kAndroidSockStream = 1;
constexpr int kAndroidSockDgram = 2;
constexpr int kAndroidSockSeqPacket = 5;
constexpr int kAndroidSockNonblock = 00004000;
constexpr int kAndroidSockCloexec = 02000000;
constexpr int kAndroidSolSocket = 1;
constexpr int kAndroidSoReuseaddr = 2;
constexpr int kAndroidSoType = 3;
constexpr int kAndroidSoError = 4;
constexpr int kAndroidSoSndbuf = 7;
constexpr int kAndroidSoRcvbuf = 8;
constexpr int kAndroidSoKeepalive = 9;
constexpr int kAndroidSoRcvtimeo = 20;
constexpr int kAndroidSoSndtimeo = 21;
constexpr int kAndroidSoPasscred = 16;
constexpr int kAndroidMsgOob = 0x1;
constexpr int kAndroidMsgPeek = 0x2;
constexpr int kAndroidMsgDontRoute = 0x4;
constexpr int kAndroidMsgDontWait = 0x40;
constexpr int kAndroidMsgEor = 0x80;
constexpr int kAndroidMsgWaitAll = 0x100;
constexpr int kAndroidMsgNoSignal = 0x4000;
constexpr uint32_t kCentralBrokerTokenMarker = UINT32_C(0x40000000);
constexpr uint32_t kCentralBrokerTokenTopMask = UINT32_C(0xc0000000);
constexpr uint32_t kAndroidSendMmsgMax = 1024;
constexpr int kAndroidFDupfd = 0;
constexpr int kAndroidFGetfd = 1;
constexpr int kAndroidFSetfd = 2;
constexpr int kAndroidFGetfl = 3;
constexpr int kAndroidFSetfl = 4;
constexpr int kAndroidFDupfdCloexec = 1030;
constexpr int kAndroidFdCloexec = 1;
constexpr int kAndroidOAppend = 1024;
constexpr int kAndroidONonblock = 2048;
constexpr int kAndroidFdSetBits = 1024;
constexpr int kAndroidFdSetWordBits = 64;
constexpr size_t kAndroidFdSetWords = kAndroidFdSetBits / kAndroidFdSetWordBits;
constexpr uint32_t kAndroidFionread = 0x541b;
constexpr uint32_t kAndroidFionbio = 0x5421;

struct AndroidSockaddrIn {
  uint16_t family;
  uint16_t port;
  uint32_t address;
  uint8_t zero[8];
};

struct AndroidSockaddrIn6 {
  uint16_t family;
  uint16_t port;
  uint32_t flowinfo;
  uint8_t address[16];
  uint32_t scope_id;
};

struct TimerState {
  std::mutex mutex;
  uint64_t generation = 0;
  bool closing = false;
  int write_fd = -1;
};

struct DnsQueryState {
  std::mutex mutex;
  std::vector<uint8_t> answer;
  int error = 0;
  int rcode = 0;
  int signal_fd = -1;
  int guest_fd = -1;
  bool complete = false;
  bool cancelled = false;
};

using EventFdState = darwin_art::eventfd::State;

struct HostFdObject {
  explicit HostFdObject(int host_fd, int host_peer_fd = -1,
                        std::shared_ptr<TimerState> timer_state = nullptr,
                        std::shared_ptr<DnsQueryState> dns_state = nullptr,
                        std::shared_ptr<EventFdState> event_state = nullptr)
      : fd(host_fd), peer_fd(host_peer_fd), timer(std::move(timer_state)),
        dns(std::move(dns_state)), event(std::move(event_state)) {}

  int fd = -1;
  // Exact object/Description lifetime, not a guest-FD-number registry. Empty
  // until the reviewed complete managed path is adopted; legacy pair stays off.
  darwin_art::bionic::scm::EndpointLease scm_endpoint;
  int peer_fd = -1;
  std::shared_ptr<TimerState> timer;
  std::shared_ptr<DnsQueryState> dns;
  std::shared_ptr<EventFdState> event;
  std::shared_ptr<darwin_art::socket::SyncFenceMerge> fence_merge;
  std::atomic<bool> pass_credentials{false};
  // A nonblocking TLS/client read can legitimately return EAGAIN many times
  // while waiting for the peer. Keep the opt-in trace useful without letting
  // that wait flood the process log.
  std::atomic<uint32_t> debug_read_eagain_logs{0};
  // A level-triggered descriptor remains readable until its owner drains it.
  // Bound readiness diagnostics as well; otherwise a single fd can emit one
  // line per Looper iteration while an opt-in network trace is running.
  std::atomic<uint32_t> debug_poll_logs{0};
};

constexpr uint32_t kSocketDebugReadEagainLogLimit = 128;
constexpr uint32_t kSocketDebugPollLogLimit = 128;
constexpr uint32_t kSocketDebugSelectLogLimit = 256;
constexpr uint32_t kSocketDebugReadChkLogLimit = 128;

std::atomic<uint32_t> g_socket_select_debug_logs{0};
std::atomic<uint32_t> g_socket_read_chk_debug_logs{0};

bool ShouldLogSocketRead(HostFdObject *socket, ssize_t result, int host_errno) {
  if (result >= 0 || (host_errno != EAGAIN && host_errno != EWOULDBLOCK)) {
    return true;
  }
  const uint32_t prior =
      socket->debug_read_eagain_logs.fetch_add(1, std::memory_order_relaxed);
  return prior < kSocketDebugReadEagainLogLimit;
}

bool ShouldLogSocketPoll(HostFdObject *socket) {
  const uint32_t prior =
      socket->debug_poll_logs.fetch_add(1, std::memory_order_relaxed);
  return prior < kSocketDebugPollLogLimit;
}

bool ShouldLogSocketSelect() {
  const uint32_t prior =
      g_socket_select_debug_logs.fetch_add(1, std::memory_order_relaxed);
  return prior < kSocketDebugSelectLogLimit;
}

bool ShouldLogSocketReadChk() {
  const uint32_t prior =
      g_socket_read_chk_debug_logs.fetch_add(1, std::memory_order_relaxed);
  return prior < kSocketDebugReadChkLogLimit;
}

struct HostPollWake {
  int read_fd = -1;
  int write_fd = -1;
};

struct AndroidTimespec64 {
  int64_t seconds;
  int64_t nanoseconds;
};

struct AndroidItimerspec {
  AndroidTimespec64 interval;
  AndroidTimespec64 value;
};

constexpr uint64_t kTimerFdSettimeRequest = UINT64_C(0x445254494d455201);
constexpr uint64_t kSyncIocMerge = UINT64_C(0xc0303e03);
constexpr uint64_t kSyncIocFileInfo = UINT64_C(0xc0383e04);

struct AndroidSyncMergeData {
  char name[32];
  int32_t fd2;
  int32_t fence;
  uint32_t flags;
  uint32_t pad;
};

struct AndroidSyncFileInfo {
  char name[32];
  int32_t status;
  uint32_t flags;
  uint32_t num_fences;
  uint32_t pad;
  uint64_t sync_fence_info;
};

struct AndroidSyncFenceInfo {
  char object_name[32];
  char driver_name[32];
  int32_t status;
  uint32_t flags;
  uint64_t timestamp_ns;
};

static_assert(sizeof(AndroidSyncFileInfo) == 56);
static_assert(sizeof(AndroidSyncFenceInfo) == 80);
static_assert(sizeof(AndroidSyncMergeData) == 48);

int BrokerFailure(DarwinArtFdBrokerStatus status);

using AndroidIovec = DarwinArtAndroidIovec;
using AndroidMsghdr = DarwinArtAndroidMsghdr;
using AndroidMmsghdr = DarwinArtAndroidMmsghdr;

static_assert(sizeof(AndroidMsghdr) == 56);
static_assert(offsetof(AndroidMsghdr, vector_count) == 24);
static_assert(sizeof(AndroidMmsghdr) == 64);
static_assert(offsetof(AndroidMmsghdr, msg_len) == 56);

struct AndroidCmsghdr {
  size_t length;
  int32_t level;
  int32_t type;
};

struct AndroidUcred {
  int32_t process_id;
  uint32_t user_id;
  uint32_t group_id;
};

// Android's timeval is two 64-bit fields. Darwin's tv_usec is a signed
// 32-bit field, so socket-option conversion must be field-wise rather than a
// raw 16-byte copy.
struct AndroidTimeval {
  int64_t seconds;
  int64_t microseconds;
};

static_assert(sizeof(AndroidTimeval) == 16);

static_assert(sizeof(AndroidUcred) == 12);

// Linux UAPI tcp_info as exposed by Android's API-35 arm64 headers. Keep the
// byte layout independent from Darwin's unrelated tcp_connection_info type.
struct AndroidTcpInfo {
  uint8_t header[8];
  uint32_t metrics[24];
  uint64_t rates[4];
  uint32_t segments[6];
  uint64_t timing[4];
  uint32_t delivery[2];
  uint64_t byte_totals[2];
  uint32_t tail[6];
  uint16_t total_rto;
  uint16_t total_rto_recoveries;
  uint32_t total_rto_time;
};

static_assert(sizeof(AndroidTcpInfo) == 248);

struct Process {
  darwin_art::socket::UnixEndpoints unix_endpoints;
  DarwinArtFdBroker *broker = nullptr;
  DarwinArtFdOwnerHandle socket_owner = 0;
  DarwinArtFdOwnerHandle pipe_owner = 0;
  DarwinArtFdOwnerHandle binder_owner = 0;
  std::mutex mutex;
  std::condition_variable changed;
  size_t active = 0;
  bool draining = false;
  std::atomic<size_t> objects{0};
  std::atomic<size_t> dns_results{0};
  std::mutex dns_mutex;
  std::unordered_map<int, std::shared_ptr<DnsQueryState>> dns_queries;
  std::atomic<size_t> async_dns_queries{0};
};

int OpenBinderDevice(int flags, uint32_t mode, int *android_errno);

std::mutex g_process_mutex;
Process *g_process = nullptr;

bool SocketDebugEnabled() {
  return std::getenv("DARWIN_ART_DEBUG_SOCKET") != nullptr;
}

uint64_t SocketDebugThreadId() {
  uint64_t thread_id = 0;
  (void)pthread_threadid_np(nullptr, &thread_id);
  return thread_id;
}

class PreserveErrno {
public:
  PreserveErrno() : saved_(errno) {}
  ~PreserveErrno() { errno = saved_; }

private:
  int saved_;
};

int32_t AndroidErrno(int error) {
  switch (error) {
  case 0:
    return 0;
  case EPERM:
    return 1;
  case ENOENT:
    return 2;
  case EINTR:
    return 4;
  case EIO:
    return 5;
  case EBADF:
    return 9;
  case EAGAIN:
    return 11;
  case ENOMEM:
    return 12;
  case EACCES:
    return 13;
  case EFAULT:
    return 14;
  case EBUSY:
    return 16;
  case EEXIST:
    return 17;
  case EINVAL:
    return 22;
  case EMFILE:
    return 24;
  case ENOTTY:
    return 25;
  case ENOSPC:
    return 28;
  case EPIPE:
    return 32;
  case EDOM:
    return 33;
  case ERANGE:
    return 34;
  case ENOSYS:
    return 38;
  case EOVERFLOW:
    return 75;
  case ENOTSOCK:
    return 88;
  case EDESTADDRREQ:
    return 89;
  case EMSGSIZE:
    return 90;
  case EPROTOTYPE:
    return 91;
  case ENOPROTOOPT:
    return 92;
  case EPROTONOSUPPORT:
    return 93;
  case ESOCKTNOSUPPORT:
    return 94;
  case EOPNOTSUPP:
    return 95;
  case EAFNOSUPPORT:
    return 97;
  case EADDRINUSE:
    return 98;
  case EADDRNOTAVAIL:
    return 99;
  case ENETDOWN:
    return 100;
  case ENETUNREACH:
    return 101;
  case ENETRESET:
    return 102;
  case ECONNABORTED:
    return 103;
  case ECONNRESET:
    return 104;
  case ENOBUFS:
    return 105;
  case EISCONN:
    return 106;
  case ENOTCONN:
    return 107;
  case ETIMEDOUT:
    return 110;
  case ECONNREFUSED:
    return 111;
  case EHOSTUNREACH:
    return 113;
  case EALREADY:
    return 114;
  case EINPROGRESS:
    return 115;
  default:
    return 5;
  }
}

template <typename T> T Fail(int error, T value) {
  darwin_art_bionic_errno_store(error);
  return value;
}

extern "C" AndroidCmsghdr *
darwin_art_bionic_socket_broker_cmsg_nxthdr(AndroidMsghdr *message,
                                            AndroidCmsghdr *current) {
  if (message == nullptr || current == nullptr || message->control == nullptr)
    return nullptr;
  constexpr size_t alignment = sizeof(size_t);
  if (current->length < sizeof(AndroidCmsghdr))
    return nullptr;
  const size_t aligned = (current->length + alignment - 1) & ~(alignment - 1);
  const uintptr_t base = reinterpret_cast<uintptr_t>(message->control);
  const uintptr_t end = base + message->control_length;
  const uintptr_t next = reinterpret_cast<uintptr_t>(current) + aligned;
  if (next < base || next > end || end - next < sizeof(AndroidCmsghdr))
    return nullptr;
  auto *result = reinterpret_cast<AndroidCmsghdr *>(next);
  if (result->length < sizeof(AndroidCmsghdr) || result->length > end - next)
    return nullptr;
  return result;
}

extern "C" unsigned
darwin_art_bionic_socket_broker_if_nametoindex(const char *name) {
  PreserveErrno preserve;
  if (name == nullptr)
    return Fail(14, 0u);
  const unsigned result = if_nametoindex(name);
  return result == 0 ? Fail(AndroidErrno(errno), 0u) : result;
}

Process *AcquireProcess() {
  std::lock_guard global(g_process_mutex);
  Process *process = g_process;
  if (process == nullptr)
    return nullptr;
  std::lock_guard local(process->mutex);
  if (process->draining)
    return nullptr;
  ++process->active;
  return process;
}

void ReleaseProcess(Process *process) {
  std::lock_guard lock(process->mutex);
  if (process->active == 0)
    std::abort();
  --process->active;
  if (process->active == 0)
    process->changed.notify_all();
}

bool RetainProcess(Process *process) {
  std::lock_guard lock(process->mutex);
  if (process->draining)
    return false;
  ++process->active;
  return true;
}

class ProcessLease {
public:
  ProcessLease() : process_(AcquireProcess()) {}
  ~ProcessLease() {
    if (process_ != nullptr)
      ReleaseProcess(process_);
  }
  Process *get() const { return process_; }

private:
  Process *process_;
};

bool TranslateDomain(int android, int *host) {
  if (android == 1) {
    *host = AF_UNIX;
    return true;
  }
  if (android == kAndroidAfInet) {
    *host = AF_INET;
    return true;
  }
  if (android == kAndroidAfInet6) {
    *host = AF_INET6;
    return true;
  }
  return false;
}

bool TranslateType(int android, int *host, bool *nonblocking) {
  constexpr int kKnown = kAndroidSockNonblock | kAndroidSockCloexec;
  if ((android & ~(kKnown | 0xf)) != 0)
    return false;
  if ((android & 0xf) == kAndroidSockStream)
    *host = SOCK_STREAM;
  else if ((android & 0xf) == kAndroidSockDgram)
    *host = SOCK_DGRAM;
  else if ((android & 0xf) == kAndroidSockSeqPacket)
    // Darwin does not implement AF_UNIX/SOCK_SEQPACKET. A connected datagram
    // pair preserves the record boundaries and descriptor-passing semantics
    // required by Android crash and Mojo channels.
    *host = SOCK_DGRAM;
  else
    return false;
  *nonblocking = (android & kAndroidSockNonblock) != 0;
  return true;
}

bool TranslateProtocol(int android, int *host) {
  if (android == 0) {
    *host = 0;
    return true;
  }
  if (android == 6) {
    *host = IPPROTO_TCP;
    return true;
  }
  if (android == 17) {
    *host = IPPROTO_UDP;
    return true;
  }
  return false;
}

bool TranslateFlags(int android, int *host) {
  constexpr int kKnown = kAndroidMsgOob | kAndroidMsgPeek |
                         kAndroidMsgDontRoute | kAndroidMsgDontWait |
                         kAndroidMsgEor | kAndroidMsgWaitAll |
                         kAndroidMsgNoSignal;
  if ((android & ~kKnown) != 0)
    return false;
  int value = 0;
  if ((android & kAndroidMsgOob) != 0)
    value |= MSG_OOB;
  if ((android & kAndroidMsgPeek) != 0)
    value |= MSG_PEEK;
  if ((android & kAndroidMsgDontRoute) != 0)
    value |= MSG_DONTROUTE;
  if ((android & kAndroidMsgDontWait) != 0)
    value |= MSG_DONTWAIT;
  if ((android & kAndroidMsgEor) != 0)
    value |= MSG_EOR;
  if ((android & kAndroidMsgWaitAll) != 0)
    value |= MSG_WAITALL;
  *host = value;
  return true;
}

bool ToHostAddress(const void *address, uint32_t length,
                   sockaddr_storage *storage, socklen_t *host_length) {
  if (address == nullptr || length < sizeof(uint16_t))
    return false;
  uint16_t family = 0;
  std::memcpy(&family, address, sizeof(family));
  std::memset(storage, 0, sizeof(*storage));
  if (family == kAndroidAfInet && length >= sizeof(AndroidSockaddrIn)) {
    AndroidSockaddrIn android{};
    std::memcpy(&android, address, sizeof(android));
    sockaddr_in host{};
    host.sin_len = sizeof(host);
    host.sin_family = AF_INET;
    host.sin_port = android.port;
    host.sin_addr.s_addr = android.address;
    std::memcpy(storage, &host, sizeof(host));
    *host_length = sizeof(host);
    return true;
  }
  if (family == kAndroidAfInet6 && length >= sizeof(AndroidSockaddrIn6)) {
    AndroidSockaddrIn6 android{};
    std::memcpy(&android, address, sizeof(android));
    sockaddr_in6 host{};
    host.sin6_len = sizeof(host);
    host.sin6_family = AF_INET6;
    host.sin6_port = android.port;
    host.sin6_flowinfo = android.flowinfo;
    std::memcpy(&host.sin6_addr, android.address, sizeof(android.address));
    host.sin6_scope_id = android.scope_id;
    std::memcpy(storage, &host, sizeof(host));
    *host_length = sizeof(host);
    return true;
  }
  return false;
}

bool FromHostAddress(const sockaddr *host, socklen_t host_length,
                     void *android_address, uint32_t capacity,
                     uint32_t *android_length) {
  if (android_length == nullptr)
    return false;
  if (host->sa_family == AF_INET && host_length >= sizeof(sockaddr_in)) {
    const auto *source = reinterpret_cast<const sockaddr_in *>(host);
    AndroidSockaddrIn result{};
    result.family = kAndroidAfInet;
    result.port = source->sin_port;
    result.address = source->sin_addr.s_addr;
    *android_length = sizeof(result);
    if (android_address != nullptr && capacity != 0) {
      std::memcpy(android_address, &result,
                  capacity < sizeof(result) ? capacity : sizeof(result));
    }
    return true;
  }
  if (host->sa_family == AF_INET6 && host_length >= sizeof(sockaddr_in6)) {
    const auto *source = reinterpret_cast<const sockaddr_in6 *>(host);
    AndroidSockaddrIn6 result{};
    result.family = kAndroidAfInet6;
    result.port = source->sin6_port;
    result.flowinfo = source->sin6_flowinfo;
    std::memcpy(result.address, &source->sin6_addr, sizeof(result.address));
    result.scope_id = source->sin6_scope_id;
    *android_length = sizeof(result);
    if (android_address != nullptr && capacity != 0) {
      std::memcpy(android_address, &result,
                  capacity < sizeof(result) ? capacity : sizeof(result));
    }
    return true;
  }
  return false;
}

bool TranslateOption(int android_level, int android_option, int *host_level,
                     int *host_option) {
  if (android_level == kAndroidSolSocket) {
    *host_level = SOL_SOCKET;
    switch (android_option) {
    case kAndroidSoReuseaddr:
      *host_option = SO_REUSEADDR;
      return true;
    case kAndroidSoType:
      *host_option = SO_TYPE;
      return true;
    case kAndroidSoError:
      *host_option = SO_ERROR;
      return true;
    case kAndroidSoSndbuf:
      *host_option = SO_SNDBUF;
      return true;
    case kAndroidSoRcvbuf:
      *host_option = SO_RCVBUF;
      return true;
    case kAndroidSoKeepalive:
      *host_option = SO_KEEPALIVE;
      return true;
    case kAndroidSoRcvtimeo:
      *host_option = SO_RCVTIMEO;
      return true;
    case kAndroidSoSndtimeo:
      *host_option = SO_SNDTIMEO;
      return true;
    default:
      return false;
    }
  }
  if (android_level == IPPROTO_TCP) {
    *host_level = IPPROTO_TCP;
    switch (android_option) {
    case 1:
      *host_option = TCP_NODELAY;
      return true;
    case 4:
      *host_option = TCP_KEEPALIVE;
      return true;
    case 5:
      *host_option = TCP_KEEPINTVL;
      return true;
    case 6:
      *host_option = TCP_KEEPCNT;
      return true;
    default:
      return false;
    }
  }
  return false;
}

intptr_t OwnerRead(void *, uint64_t object, void *bytes, size_t count,
                   int *android_errno) {
  auto *socket = reinterpret_cast<HostFdObject *>(object);
  if (socket->event != nullptr)
    return darwin_art::eventfd::Read(socket->event, socket->fd, bytes, count,
                                     android_errno);
  const auto started = std::chrono::steady_clock::now();
  const int status_flags = fcntl(socket->fd, F_GETFL);
  const ssize_t result = recv(socket->fd, bytes, count, 0);
  const int host_errno = result < 0 ? errno : 0;
  if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
    const auto elapsed_us =
        std::chrono::duration_cast<std::chrono::microseconds>(
            std::chrono::steady_clock::now() - started)
            .count();
    if (elapsed_us >= 100'000) {
      std::fprintf(stderr,
                   "DARWIN_ART slow-broker-read host_fd=%d count=%zu "
                   "host_flags=0x%x result=%zd errno=%d elapsed_us=%lld\n",
                   socket->fd, count, status_flags, result,
                   result < 0 ? errno : 0, static_cast<long long>(elapsed_us));
    }
  }
  if (SocketDebugEnabled() && ShouldLogSocketRead(socket, result, host_errno)) {
    uint32_t control = 0;
    if (result == sizeof(control))
      std::memcpy(&control, bytes, sizeof(control));
    std::fprintf(stderr,
                 "DARWIN socket read op=read pid=%d host_fd=%d count=%zu "
                 "result=%zd control=%u host_errno=%d android_errno=%d\n",
                 getpid(), socket->fd, count, result, control, host_errno,
                 result < 0 ? AndroidErrno(host_errno) : 0);
  }
  *android_errno = result < 0 ? AndroidErrno(errno) : 0;
  return result;
}

intptr_t OwnerWrite(void *, uint64_t object, const void *bytes, size_t count,
                    int *android_errno) {
  auto *socket = reinterpret_cast<HostFdObject *>(object);
  if (socket->event != nullptr)
    return darwin_art::eventfd::Write(socket->event, bytes, count,
                                      android_errno);
  const ssize_t result = send(socket->fd, bytes, count, 0);
  if (SocketDebugEnabled()) {
    uint32_t control = 0;
    if (count == sizeof(control))
      std::memcpy(&control, bytes, sizeof(control));
    std::fprintf(stderr,
                 "DARWIN socket write pid=%d host_fd=%d count=%zu result=%zd "
                 "control=%u host_errno=%d\n",
                 getpid(), socket->fd, count, result, control,
                 result < 0 ? errno : 0);
  }
  *android_errno = result < 0 ? AndroidErrno(errno) : 0;
  return result;
}

int OwnerPoll(void *, uint64_t object, int16_t events, int16_t *revents,
              int *android_errno) {
  auto *socket = reinterpret_cast<HostFdObject *>(object);
  int signal_errno = 0;
  if (darwin_art::eventfd::EnsureSignaled(socket->event, &signal_errno) != 0) {
    *android_errno = AndroidErrno(signal_errno);
    *revents = 0;
    return -1;
  }
  pollfd descriptor{socket->fd, events, 0};
  const int result = poll(&descriptor, 1, 0);
  if (result < 0) {
    *android_errno = AndroidErrno(errno);
    return -1;
  }
  *revents = descriptor.revents;
  if (SocketDebugEnabled() && (result != 0 || descriptor.revents != 0) &&
      ShouldLogSocketPoll(socket)) {
    std::fprintf(
        stderr,
        "DARWIN socket poll host_fd=%d events=0x%x result=%d revents=0x%x\n",
        socket->fd, static_cast<unsigned>(static_cast<uint16_t>(events)),
        result,
        static_cast<unsigned>(static_cast<uint16_t>(descriptor.revents)));
  }
  *android_errno = 0;
  return result;
}

int OwnerPollMany(void *, const uint64_t *objects, const int16_t *events,
                  int16_t *revents, size_t count, int timeout_ms,
                  int *android_errno) {
  static_assert(POLLIN == 0x0001 && POLLPRI == 0x0002 && POLLOUT == 0x0004 &&
                POLLERR == 0x0008 && POLLHUP == 0x0010 && POLLNVAL == 0x0020);
  if ((count != 0 &&
       (objects == nullptr || events == nullptr || revents == nullptr)) ||
      timeout_ms < -1) {
    *android_errno = 22;
    return -1;
  }
  std::vector<pollfd> descriptors(count);
  for (size_t index = 0; index < count; ++index) {
    const auto *object = reinterpret_cast<const HostFdObject *>(objects[index]);
    int signal_errno = 0;
    if (darwin_art::eventfd::EnsureSignaled(object->event, &signal_errno) !=
        0) {
      *android_errno = AndroidErrno(signal_errno);
      return -1;
    }
    descriptors[index] = pollfd{object->fd, events[index], 0};
  }
  const int result =
      poll(descriptors.data(), static_cast<nfds_t>(count), timeout_ms);
  const int saved = errno;
  if (result >= 0) {
    for (size_t index = 0; index < count; ++index) {
      revents[index] = descriptors[index].revents;
      if (SocketDebugEnabled() && descriptors[index].revents != 0 &&
          ShouldLogSocketPoll(
              reinterpret_cast<HostFdObject *>(objects[index]))) {
        std::fprintf(
            stderr,
            "DARWIN socket poll_many host_fd=%d events=0x%x revents=0x%x "
            "timeout_ms=%d\n",
            descriptors[index].fd,
            static_cast<unsigned>(static_cast<uint16_t>(events[index])),
            static_cast<unsigned>(
                static_cast<uint16_t>(descriptors[index].revents)),
            timeout_ms);
      }
    }
  }
  *android_errno = result < 0 ? AndroidErrno(saved) : 0;
  return result;
}

uint64_t OwnerCreatePollWake(void *, int *android_errno) {
  int descriptors[2] = {-1, -1};
  if (native_fd::CreateCloseOnExecPipe(descriptors) != 0) {
    *android_errno = AndroidErrno(errno);
    return 0;
  }
  bool configured = true;
  for (int descriptor : descriptors) {
    const int flags = fcntl(descriptor, F_GETFL);
    const int fd_flags = fcntl(descriptor, F_GETFD);
    configured = configured && flags >= 0 && fd_flags >= 0 &&
                 fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) == 0 &&
                 fcntl(descriptor, F_SETFD, fd_flags | FD_CLOEXEC) == 0;
  }
  if (!configured) {
    const int saved = errno;
    (void)close(descriptors[0]);
    (void)close(descriptors[1]);
    *android_errno = AndroidErrno(saved);
    return 0;
  }
  auto *wake = new (std::nothrow) HostPollWake{descriptors[0], descriptors[1]};
  if (wake == nullptr) {
    (void)close(descriptors[0]);
    (void)close(descriptors[1]);
    *android_errno = 12;
    return 0;
  }
  *android_errno = 0;
  return reinterpret_cast<uint64_t>(wake);
}

int OwnerSignalPollWake(void *, uint64_t object, int *android_errno) {
  auto *wake = reinterpret_cast<HostPollWake *>(object);
  const uint8_t signal = 1;
  const ssize_t result = write(wake->write_fd, &signal, sizeof(signal));
  if (result < 0 && errno != EAGAIN) {
    *android_errno = AndroidErrno(errno);
    return -1;
  }
  *android_errno = 0;
  return 0;
}

int OwnerClosePollWake(void *, uint64_t object, int *android_errno) {
  auto *wake = reinterpret_cast<HostPollWake *>(object);
  const int read_result = close(wake->read_fd);
  const int saved = errno;
  const int write_result = close(wake->write_fd);
  const int write_saved = errno;
  delete wake;
  if (read_result < 0 || write_result < 0) {
    *android_errno = AndroidErrno(read_result < 0 ? saved : write_saved);
    return -1;
  }
  *android_errno = 0;
  return 0;
}

int OwnerPollManyWithWake(void *, const uint64_t *objects,
                          const int16_t *events, int16_t *revents, size_t count,
                          int timeout_ms, uint64_t wake_object, int *woke,
                          int *android_errno) {
  if ((count != 0 &&
       (objects == nullptr || events == nullptr || revents == nullptr)) ||
      wake_object == 0 || woke == nullptr || timeout_ms < -1) {
    *android_errno = 22;
    return -1;
  }
  auto *wake = reinterpret_cast<HostPollWake *>(wake_object);
  std::vector<pollfd> descriptors(count + 1);
  for (size_t index = 0; index < count; ++index) {
    const auto *object = reinterpret_cast<const HostFdObject *>(objects[index]);
    int signal_errno = 0;
    if (darwin_art::eventfd::EnsureSignaled(object->event, &signal_errno) !=
        0) {
      *android_errno = AndroidErrno(signal_errno);
      return -1;
    }
    descriptors[index] = pollfd{object->fd, events[index], 0};
  }
  descriptors[count] = pollfd{wake->read_fd, POLLIN, 0};
  const int result = poll(descriptors.data(),
                          static_cast<nfds_t>(descriptors.size()), timeout_ms);
  const int saved = errno;
  *woke = result >= 0 && (descriptors[count].revents & POLLIN) != 0 ? 1 : 0;
  if (*woke != 0) {
    uint8_t bytes[64];
    while (read(wake->read_fd, bytes, sizeof(bytes)) > 0) {
    }
  }
  if (result >= 0) {
    for (size_t index = 0; index < count; ++index)
      revents[index] = descriptors[index].revents;
  }
  *android_errno = result < 0 ? AndroidErrno(saved) : 0;
  return result;
}

int OwnerSetStatusFlags(void *, uint64_t object, int flags,
                        int *android_errno) {
  auto *descriptor = reinterpret_cast<HostFdObject *>(object);
  const int current = fcntl(descriptor->fd, F_GETFL);
  if (current < 0) {
    *android_errno = AndroidErrno(errno);
    return -1;
  }
  int updated = current & ~(O_APPEND | O_NONBLOCK);
  if ((flags & kAndroidOAppend) != 0)
    updated |= O_APPEND;
  if ((flags & kAndroidONonblock) != 0)
    updated |= O_NONBLOCK;
  const int result = fcntl(descriptor->fd, F_SETFL, updated);
  const int saved = errno;
  if (SocketDebugEnabled()) {
    std::fprintf(stderr,
                 "DARWIN fd set-status host_fd=%d android_flags=0x%x "
                 "host_flags=0x%x result=%d host_errno=%d\n",
                 descriptor->fd, flags, updated, result,
                 result < 0 ? saved : 0);
  }
  *android_errno = result < 0 ? AndroidErrno(saved) : 0;
  return result;
}

intptr_t PipeOwnerRead(void *, uint64_t object, void *bytes, size_t count,
                       int *android_errno) {
  auto *pipe = reinterpret_cast<HostFdObject *>(object);
  if (pipe->event != nullptr)
    return darwin_art::eventfd::Read(pipe->event, pipe->fd, bytes, count,
                                     android_errno);
  const auto started = std::chrono::steady_clock::now();
  const int status_flags = fcntl(pipe->fd, F_GETFL);
  const ssize_t result = read(pipe->fd, bytes, count);
  if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
    const auto elapsed_us =
        std::chrono::duration_cast<std::chrono::microseconds>(
            std::chrono::steady_clock::now() - started)
            .count();
    if (elapsed_us >= 100'000) {
      std::fprintf(stderr,
                   "DARWIN_ART slow-broker-pipe-read host_fd=%d count=%zu "
                   "host_flags=0x%x result=%zd errno=%d elapsed_us=%lld\n",
                   pipe->fd, count, status_flags, result,
                   result < 0 ? errno : 0, static_cast<long long>(elapsed_us));
    }
  }
  *android_errno = result < 0 ? AndroidErrno(errno) : 0;
  return result;
}

intptr_t PipeOwnerWrite(void *, uint64_t object, const void *bytes,
                        size_t count, int *android_errno) {
  auto *pipe = reinterpret_cast<HostFdObject *>(object);
  if (pipe->event != nullptr)
    return darwin_art::eventfd::Write(pipe->event, bytes, count, android_errno);
  const ssize_t result =
      write(pipe->peer_fd >= 0 ? pipe->peer_fd : pipe->fd, bytes, count);
  *android_errno = result < 0 ? AndroidErrno(errno) : 0;
  return result;
}

int OwnerIoctl(void *, uint64_t object, uint64_t request, void *argument,
               int *android_errno) {
  auto *descriptor = reinterpret_cast<HostFdObject *>(object);
  if (request == kAndroidFionread) {
    if (argument == nullptr) {
      *android_errno = 14;
      return -1;
    }
    const int result = ioctl(descriptor->fd, FIONREAD, argument);
    *android_errno = result < 0 ? AndroidErrno(errno) : 0;
    return result;
  }
  if (request == kSyncIocFileInfo && argument != nullptr) {
    int available = 0;
    struct stat status{};
    // Darwin has no Linux sync_file object. The EGL bridge publishes a pipe
    // whose write end is completed by an MTLSharedEvent listener. The marker
    // survives dup and SCM_RIGHTS, while FIONREAD reports pending/signaled
    // state without consuming the readiness used by libsync's sync_wait.
    if (fstat(descriptor->fd, &status) == 0 && S_ISFIFO(status.st_mode) &&
        ioctl(descriptor->fd, FIONREAD, &available) == 0 &&
        (available == 0 || available == static_cast<int>(sizeof(uint64_t)))) {
      pollfd readiness{descriptor->fd, POLLIN, 0};
      const bool failed = available == 0 && poll(&readiness, 1, 0) > 0 &&
                          (readiness.revents & (POLLHUP | POLLERR | POLLNVAL));
      const int fence_status =
          failed ? -5 : (available == static_cast<int>(sizeof(uint64_t)) ? 1 : 0);
      auto *info = static_cast<AndroidSyncFileInfo *>(argument);
      std::memset(info->name, 0, sizeof(info->name));
      std::memcpy(info->name, "darwin-metal", sizeof("darwin-metal") - 1);
      info->status = fence_status;
      info->flags = 0;
      info->pad = 0;
      if (info->sync_fence_info == 0) {
        info->num_fences = 1;
      } else {
        auto *fence = reinterpret_cast<AndroidSyncFenceInfo *>(
            static_cast<uintptr_t>(info->sync_fence_info));
        std::memset(fence, 0, sizeof(*fence));
        std::memcpy(fence->object_name, "metal-command-buffer",
                    sizeof("metal-command-buffer") - 1);
        std::memcpy(fence->driver_name, "darwin-angle",
                    sizeof("darwin-angle") - 1);
        fence->status = fence_status;
        struct timespec now{};
        if (fence_status == 1) {
          (void)clock_gettime(CLOCK_MONOTONIC, &now);
          fence->timestamp_ns =
              static_cast<uint64_t>(now.tv_sec) * UINT64_C(1000000000) +
              static_cast<uint64_t>(now.tv_nsec);
        }
        info->num_fences = 1;
      }
      *android_errno = 0;
      return 0;
    }
  }
  if (request == kTimerFdSettimeRequest && descriptor->timer != nullptr &&
      argument != nullptr) {
    const auto specification = *static_cast<AndroidItimerspec *>(argument);
    auto valid = [](const AndroidTimespec64 &time) {
      return time.seconds >= 0 && time.nanoseconds >= 0 &&
             time.nanoseconds < 1000000000;
    };
    if (!valid(specification.value) || !valid(specification.interval)) {
      *android_errno = 22;
      return -1;
    }
    const auto first =
        std::chrono::seconds(specification.value.seconds) +
        std::chrono::nanoseconds(specification.value.nanoseconds);
    const auto interval =
        std::chrono::seconds(specification.interval.seconds) +
        std::chrono::nanoseconds(specification.interval.nanoseconds);
    const auto state = descriptor->timer;
    uint64_t generation = 0;
    {
      std::lock_guard lock(state->mutex);
      generation = ++state->generation;
    }
    if (first != std::chrono::nanoseconds::zero()) {
      std::thread([state, generation, first, interval] {
        auto deadline = std::chrono::steady_clock::now() + first;
        for (;;) {
          std::this_thread::sleep_until(deadline);
          std::lock_guard lock(state->mutex);
          if (state->closing || state->generation != generation)
            return;
          const uint64_t expiration = 1;
          (void)write(state->write_fd, &expiration, sizeof(expiration));
          if (interval == std::chrono::nanoseconds::zero())
            return;
          deadline += interval;
        }
      }).detach();
    }
    *android_errno = 0;
    return 0;
  }
  *android_errno = 25;
  return -1;
}

int OwnerClose(void *context, uint64_t object, int *android_errno);

int PublishMergedFence(
    void *context, int read_fd,
    std::shared_ptr<darwin_art::socket::SyncFenceMerge> merge, int *error) {
  auto *process = static_cast<Process *>(context);
  auto *object = new (std::nothrow) HostFdObject{read_fd};
  if (!object) {
    merge->Cancel();
    close(read_fd);
    *error = 12;
    return -1;
  }
  object->fence_merge = std::move(merge);
  process->objects.fetch_add(1, std::memory_order_release);
  int guest = -1;
  const auto status = darwin_art_fd_broker_publish_with_flags(
      process->broker, process->pipe_owner, reinterpret_cast<uint64_t>(object),
      0, DARWIN_ART_FD_CLOEXEC, &guest);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    int ignored = 0;
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(object), &ignored);
    *error = BrokerFailure(status);
    return -1;
  }
  *error = 0;
  return guest;
}

extern "C" int darwin_art_bionic_fd_broker_ioctl_dispatch(
    int fd, uint32_t request, void *argument, int *handled, int *result,
    int *android_errno) {
  PreserveErrno preserve;
  if (handled == nullptr || result == nullptr || android_errno == nullptr)
    return -1;
  *handled = 0;
  *result = -1;
  *android_errno = 0;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return 0;
  if (request == kSyncIocMerge) {
    int flags = 0;
    const auto status =
        darwin_art_fd_broker_get_status_flags(process->broker, fd, &flags);
    if (status == DARWIN_ART_FD_BROKER_STALE)
      return 0;
    *handled = 1;
    if (status != DARWIN_ART_FD_BROKER_OK) {
      *android_errno = BrokerFailure(status);
      return 0;
    }
    if (!argument) {
      *android_errno = 14;
      return 0;
    }
    auto *data = static_cast<AndroidSyncMergeData *>(argument);
    if (data->flags != 0 || data->pad != 0) {
      *android_errno = 22;
      return 0;
    }
    const int merged = darwin_art::socket::MergeBrokerFences(
        process->broker, fd, data->fd2, process, &PublishMergedFence,
        android_errno);
    if (merged >= 0) {
      data->fence = merged;
      *result = 0;
    }
    return 0;
  }
  if (request == kAndroidFionbio) {
    int flags = 0;
    DarwinArtFdBrokerStatus status =
        darwin_art_fd_broker_get_status_flags(process->broker, fd, &flags);
    if (status == DARWIN_ART_FD_BROKER_WRONG_KIND ||
        status == DARWIN_ART_FD_BROKER_WRONG_OWNER ||
        status == DARWIN_ART_FD_BROKER_STALE) {
      return 0;
    }
    *handled = 1;
    if (status != DARWIN_ART_FD_BROKER_OK) {
      *android_errno = BrokerFailure(status);
      return 0;
    }
    if (argument == nullptr) {
      *android_errno = 14;
      return 0;
    }
    if (*static_cast<int *>(argument) != 0) {
      flags |= kAndroidONonblock;
    } else {
      flags &= ~kAndroidONonblock;
    }
    DarwinArtFdIoResult io_result{};
    status = darwin_art_fd_broker_set_status_flags_io(process->broker, fd,
                                                      flags, &io_result);
    if (status != DARWIN_ART_FD_BROKER_OK) {
      *android_errno = BrokerFailure(status);
      return 0;
    }
    *result = static_cast<int>(io_result.value);
    *android_errno = io_result.android_errno;
    return 0;
  }
  DarwinArtFdIoResult io_result{};
  auto status = darwin_art_fd_broker_ioctl(
      process->broker, fd, DARWIN_ART_FD_PIPE, request, argument, &io_result);
  if (status == DARWIN_ART_FD_BROKER_WRONG_KIND ||
      status == DARWIN_ART_FD_BROKER_WRONG_OWNER) {
    status =
        darwin_art_fd_broker_ioctl(process->broker, fd, DARWIN_ART_FD_SOCKET,
                                   request, argument, &io_result);
  }
  if (status == DARWIN_ART_FD_BROKER_WRONG_KIND ||
      status == DARWIN_ART_FD_BROKER_WRONG_OWNER) {
    status =
        darwin_art_fd_broker_ioctl(process->broker, fd, DARWIN_ART_FD_BINDER,
                                   request, argument, &io_result);
  }
  if (status == DARWIN_ART_FD_BROKER_WRONG_KIND ||
      status == DARWIN_ART_FD_BROKER_WRONG_OWNER ||
      status == DARWIN_ART_FD_BROKER_STALE) {
    return 0;
  }
  *handled = 1;
  if (status != DARWIN_ART_FD_BROKER_OK) {
    *android_errno = BrokerFailure(status);
    return 0;
  }
  *result = static_cast<int>(io_result.value);
  *android_errno = io_result.android_errno;
  return 0;
}

extern "C" int darwin_art_bionic_socket_broker_ioctl_dispatch(
    int fd, uint32_t request, void *argument, int *handled, int *result,
    int *android_errno) {
  return darwin_art_bionic_fd_broker_ioctl_dispatch(
      fd, request, argument, handled, result, android_errno);
}

extern "C" DarwinArtFdBrokerStatus
darwin_art_bionic_binder_fd_install_owner(
    const DarwinArtFdOwnerV1 *callbacks, DarwinArtFdOwnerHandle *owner) {
  if (callbacks == nullptr || owner == nullptr)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return DARWIN_ART_FD_BROKER_DRAINING;
  std::lock_guard lock(process->mutex);
  if (process->binder_owner != 0)
    return DARWIN_ART_FD_BROKER_ALREADY_EXISTS;
  const DarwinArtFdBrokerStatus status = darwin_art_fd_broker_install_owner(
      process->broker, DARWIN_ART_FD_BINDER, callbacks, owner);
  if (status == DARWIN_ART_FD_BROKER_OK) {
    process->binder_owner = *owner;
    if (darwin_art_bionic_fs_bind_binder_device_open(&OpenBinderDevice) != 0) {
      process->binder_owner = 0;
      (void)darwin_art_fd_broker_uninstall_owner(process->broker, *owner);
      *owner = 0;
      return DARWIN_ART_FD_BROKER_BUSY;
    }
  }
  return status;
}

extern "C" DarwinArtFdBrokerStatus darwin_art_bionic_binder_fd_publish(
    DarwinArtFdOwnerHandle owner, uint64_t object, int *guest_fd) {
  if (owner == 0 || guest_fd == nullptr)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return DARWIN_ART_FD_BROKER_DRAINING;
  {
    std::lock_guard lock(process->mutex);
    if (process->binder_owner != owner)
      return DARWIN_ART_FD_BROKER_WRONG_OWNER;
  }
  return darwin_art_fd_broker_publish(process->broker, owner, object, guest_fd);
}

extern "C" DarwinArtFdBrokerStatus
darwin_art_bionic_binder_fd_uninstall_owner(DarwinArtFdOwnerHandle owner) {
  if (owner == 0)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return DARWIN_ART_FD_BROKER_DRAINING;
  {
    std::lock_guard lock(process->mutex);
    if (process->binder_owner != owner)
      return DARWIN_ART_FD_BROKER_WRONG_OWNER;
  }
  if (darwin_art_bionic_fs_bind_binder_device_open(nullptr) != 0)
    return DARWIN_ART_FD_BROKER_BUSY;
  DarwinArtFdBrokerStatus status =
      darwin_art_fd_broker_wait_owner_quiescent(process->broker, owner);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    (void)darwin_art_bionic_fs_bind_binder_device_open(&OpenBinderDevice);
    return status;
  }
  status = darwin_art_fd_broker_uninstall_owner(process->broker, owner);
  if (status == DARWIN_ART_FD_BROKER_OK) {
    std::lock_guard lock(process->mutex);
    if (process->binder_owner != owner)
      std::abort();
    process->binder_owner = 0;
  }
  return status;
}

int OwnerClose(void *context, uint64_t object, int *android_errno) {
  auto *process = static_cast<Process *>(context);
  auto *socket = reinterpret_cast<HostFdObject *>(object);
  if (socket->fence_merge)
    socket->fence_merge->Cancel();
  darwin_art::eventfd::Retire(socket->event);
  if (socket->timer != nullptr) {
    std::lock_guard lock(socket->timer->mutex);
    socket->timer->closing = true;
    ++socket->timer->generation;
  }
  if (socket->dns != nullptr) {
    {
      std::lock_guard lock(process->dns_mutex);
      auto found = process->dns_queries.find(socket->dns->guest_fd);
      if (found != process->dns_queries.end() && found->second == socket->dns) {
        process->dns_queries.erase(found);
        const size_t previous =
            process->async_dns_queries.fetch_sub(1, std::memory_order_acq_rel);
        if (previous == 0)
          std::abort();
      }
    }
    std::lock_guard lock(socket->dns->mutex);
    socket->dns->cancelled = true;
    socket->dns->signal_fd = -1;
  }
  const int result = close(socket->fd);
  const int saved = errno;
  if (socket->peer_fd >= 0)
    (void)close(socket->peer_fd);
  delete socket;
  if (process->objects.fetch_sub(1, std::memory_order_acq_rel) == 0)
    std::abort();
  *android_errno = result < 0 ? AndroidErrno(saved) : 0;
  return result;
}

int OwnerExportHostFd(void *, uint64_t object, int *host_fd,
                      int *android_errno) {
  auto *descriptor = reinterpret_cast<HostFdObject *>(object);
  if (host_fd == nullptr || descriptor == nullptr || descriptor->fd < 0) {
    *android_errno = 9;
    return -1;
  }
  const int duplicate = dup(descriptor->fd);
  if (duplicate < 0) {
    *android_errno = AndroidErrno(errno);
    return -1;
  }
  *host_fd = duplicate;
  *android_errno = 0;
  return 0;
}

intptr_t OwnerSocketOperation(void *context, uint64_t object,
                              const DarwinArtFdSocketRequestV1 *request,
                              DarwinArtFdSocketAcceptResultV1 *accepted_result,
                              int *android_errno) {
  auto *process = static_cast<Process *>(context);
  auto *socket = reinterpret_cast<HostFdObject *>(object);
  if (request->operation == DARWIN_ART_FD_SOCKET_BIND) {
    sockaddr_storage storage{};
    socklen_t length = 0;
    if (!ToHostAddress(request->address, request->address_length, &storage,
                       &length)) {
      *android_errno = 22;
      return -1;
    }
    const int result =
        bind(socket->fd, reinterpret_cast<const sockaddr *>(&storage), length);
    *android_errno = result < 0 ? AndroidErrno(errno) : 0;
    return result;
  }
  if (request->operation == DARWIN_ART_FD_SOCKET_CONNECT) {
    sockaddr_storage storage{};
    socklen_t length = 0;
    const auto unix_address = darwin_art::socket::ParseUnixAddress(
        request->address, request->address_length);
    if (unix_address) {
      const auto endpoint = process->unix_endpoints.Resolve(*unix_address);
      if (!endpoint) {
        *android_errno = 2;
        return -1;
      }
      std::memcpy(&storage, &*endpoint, sizeof(*endpoint));
      length = endpoint->sun_len;
    } else if (!ToHostAddress(request->address, request->address_length,
                              &storage, &length)) {
      *android_errno = 22;
      return -1;
    }
    const int result =
        connect(socket->fd, reinterpret_cast<sockaddr *>(&storage), length);
    if (SocketDebugEnabled()) {
      char address_text[INET6_ADDRSTRLEN] = {};
      uint16_t port = 0;
      if (storage.ss_family == AF_INET) {
        const auto *address = reinterpret_cast<const sockaddr_in *>(&storage);
        (void)inet_ntop(AF_INET, &address->sin_addr, address_text,
                        sizeof(address_text));
        port = ntohs(address->sin_port);
      } else if (storage.ss_family == AF_INET6) {
        const auto *address = reinterpret_cast<const sockaddr_in6 *>(&storage);
        (void)inet_ntop(AF_INET6, &address->sin6_addr, address_text,
                        sizeof(address_text));
        port = ntohs(address->sin6_port);
      }
      std::fprintf(stderr,
                   "DARWIN socket connect host_fd=%d address=%s port=%u "
                   "result=%d host_errno=%d android_errno=%d\n",
                   socket->fd, address_text, port, result,
                   result < 0 ? errno : 0,
                   result < 0 ? AndroidErrno(errno) : 0);
    }
    *android_errno = result < 0 ? AndroidErrno(errno) : 0;
    return result;
  }
  if (request->operation == DARWIN_ART_FD_SOCKET_LISTEN) {
    const int result = listen(socket->fd, request->argument);
    *android_errno = result < 0 ? AndroidErrno(errno) : 0;
    return result;
  }
  if (request->operation == DARWIN_ART_FD_SOCKET_GETPEERNAME ||
      request->operation == DARWIN_ART_FD_SOCKET_GETSOCKNAME) {
    sockaddr_storage storage{};
    socklen_t length = sizeof(storage);
    const int result =
        request->operation == DARWIN_ART_FD_SOCKET_GETPEERNAME
            ? getpeername(socket->fd, reinterpret_cast<sockaddr *>(&storage),
                          &length)
            : getsockname(socket->fd, reinterpret_cast<sockaddr *>(&storage),
                          &length);
    if (result < 0) {
      *android_errno = AndroidErrno(errno);
      return -1;
    }
    if (!FromHostAddress(reinterpret_cast<const sockaddr *>(&storage), length,
                         request->output_address,
                         request->output_address_capacity,
                         request->output_address_length)) {
      *android_errno = 14;
      return -1;
    }
    *android_errno = 0;
    return 0;
  }
  if (request->operation == DARWIN_ART_FD_SOCKET_ACCEPT4) {
    if ((request->flags & ~(kAndroidSockNonblock | kAndroidSockCloexec)) != 0) {
      *android_errno = 22;
      return -1;
    }
    sockaddr_storage storage{};
    socklen_t length = sizeof(storage);
    const int accepted =
        accept(socket->fd, reinterpret_cast<sockaddr *>(&storage), &length);
    if (accepted < 0) {
      *android_errno = AndroidErrno(errno);
      return -1;
    }
    const bool nonblocking = (request->flags & kAndroidSockNonblock) != 0;
    if (fcntl(accepted, F_SETFD, FD_CLOEXEC) < 0 ||
        (nonblocking &&
         fcntl(accepted, F_SETFL, fcntl(accepted, F_GETFL) | O_NONBLOCK) < 0)) {
      const int error = AndroidErrno(errno);
      (void)close(accepted);
      *android_errno = error;
      return -1;
    }
    if (request->output_address != nullptr &&
        !FromHostAddress(reinterpret_cast<const sockaddr *>(&storage), length,
                         request->output_address,
                         request->output_address_capacity,
                         request->output_address_length)) {
      (void)close(accepted);
      *android_errno = 14;
      return -1;
    }
    auto *accepted_object = new (std::nothrow) HostFdObject{accepted};
    if (accepted_object == nullptr) {
      (void)close(accepted);
      *android_errno = 12;
      return -1;
    }
    process->objects.fetch_add(1, std::memory_order_release);
    accepted_result->object = reinterpret_cast<uint64_t>(accepted_object);
    accepted_result->kind = DARWIN_ART_FD_SOCKET;
    accepted_result->status_flags =
        nonblocking ? DARWIN_ART_FD_STATUS_NONBLOCK : 0;
    accepted_result->descriptor_flags =
        (request->flags & kAndroidSockCloexec) != 0 ? DARWIN_ART_FD_CLOEXEC : 0;
    *android_errno = 0;
    return 0;
  }
  int flags = 0;
  if ((request->operation == DARWIN_ART_FD_SOCKET_SEND ||
       request->operation == DARWIN_ART_FD_SOCKET_RECV) &&
      !TranslateFlags(request->flags, &flags)) {
    *android_errno = 22;
    return -1;
  }
  if (request->operation == DARWIN_ART_FD_SOCKET_SEND) {
    const ssize_t result =
        send(socket->fd, request->input_bytes, request->byte_count, flags);
    if (SocketDebugEnabled()) {
      std::fprintf(stderr,
                   "DARWIN socket send host_fd=%d count=%zu flags=0x%x "
                   "result=%zd host_errno=%d\n",
                   socket->fd, request->byte_count, flags, result,
                   result < 0 ? errno : 0);
    }
    *android_errno = result < 0 ? AndroidErrno(errno) : 0;
    return result;
  }
  if (request->operation == DARWIN_ART_FD_SOCKET_RECV) {
    const ssize_t result =
        recv(socket->fd, request->output_bytes, request->byte_count, flags);
    const int host_errno = result < 0 ? errno : 0;
    if (SocketDebugEnabled() &&
        ShouldLogSocketRead(socket, result, host_errno)) {
      std::fprintf(stderr,
                   "DARWIN socket recv op=recv host_fd=%d count=%zu "
                   "flags=0x%x result=%zd host_errno=%d\n",
                   socket->fd, request->byte_count, flags, result, host_errno);
    }
    *android_errno = result < 0 ? AndroidErrno(errno) : 0;
    return result;
  }
  if (request->operation == DARWIN_ART_FD_SOCKET_SHUTDOWN) {
    if (request->argument < 0 || request->argument > 2) {
      *android_errno = 22;
      return -1;
    }
    const int result = shutdown(socket->fd, request->argument);
    *android_errno = result < 0 ? AndroidErrno(errno) : 0;
    return result;
  }
  if (request->operation == DARWIN_ART_FD_SOCKET_SENDTO) {
    if (!TranslateFlags(request->flags, &flags)) {
      *android_errno = 22;
      return -1;
    }
    ssize_t result = -1;
    if (request->address == nullptr && request->address_length == 0) {
      // Bionic implements send() in terms of sendto(..., nullptr, 0). POSIX
      // requires that form to use the peer of an already-connected socket.
      result =
          send(socket->fd, request->input_bytes, request->byte_count, flags);
    } else {
      sockaddr_storage storage{};
      socklen_t length = 0;
      if (!ToHostAddress(request->address, request->address_length, &storage,
                         &length)) {
        *android_errno = 22;
        return -1;
      }
      result =
          sendto(socket->fd, request->input_bytes, request->byte_count, flags,
                 reinterpret_cast<const sockaddr *>(&storage), length);
    }
    if (SocketDebugEnabled()) {
      std::fprintf(stderr,
                   "DARWIN socket sendto host_fd=%d connected=%d count=%zu "
                   "flags=0x%x result=%zd host_errno=%d\n",
                   socket->fd, request->address == nullptr, request->byte_count,
                   flags, result, result < 0 ? errno : 0);
    }
    *android_errno = result < 0 ? AndroidErrno(errno) : 0;
    return result;
  }
  if (request->operation == DARWIN_ART_FD_SOCKET_RECVFROM) {
    if (!TranslateFlags(request->flags, &flags)) {
      *android_errno = 22;
      return -1;
    }
    sockaddr_storage storage{};
    socklen_t length = sizeof(storage);
    const bool wants_address = request->output_address != nullptr;
    const ssize_t result =
        wants_address
            ? recvfrom(socket->fd, request->output_bytes, request->byte_count,
                       flags, reinterpret_cast<sockaddr *>(&storage), &length)
            : recv(socket->fd, request->output_bytes, request->byte_count,
                   flags);
    const int host_errno = result < 0 ? errno : 0;
    if (SocketDebugEnabled() &&
        ShouldLogSocketRead(socket, result, host_errno)) {
      std::fprintf(stderr,
                   "DARWIN socket recvfrom op=recvfrom host_fd=%d "
                   "address=%d count=%zu flags=0x%x result=%zd "
                   "host_errno=%d\n",
                   socket->fd, wants_address ? 1 : 0, request->byte_count,
                   flags, result, host_errno);
    }
    if (result < 0) {
      *android_errno = AndroidErrno(host_errno);
      return -1;
    }
    if (wants_address &&
        !FromHostAddress(reinterpret_cast<const sockaddr *>(&storage), length,
                         request->output_address,
                         request->output_address_capacity,
                         request->output_address_length)) {
      *android_errno = 97;
      return -1;
    }
    *android_errno = 0;
    return result;
  }
  if (request->operation == DARWIN_ART_FD_SOCKET_GETSOCKOPT ||
      request->operation == DARWIN_ART_FD_SOCKET_SETSOCKOPT) {
    if (request->level == kAndroidSolSocket &&
        request->option == kAndroidSoPasscred) {
      if (request->operation == DARWIN_ART_FD_SOCKET_SETSOCKOPT) {
        if (request->option_input == nullptr ||
            request->option_input_length < sizeof(int32_t)) {
          *android_errno = 22;
          return -1;
        }
        int32_t enabled = 0;
        std::memcpy(&enabled, request->option_input, sizeof(enabled));
        socket->pass_credentials.store(enabled != 0, std::memory_order_release);
        if (SocketDebugEnabled()) {
          std::fprintf(stderr,
                       "DARWIN socket: SO_PASSCRED host_fd=%d enabled=%d\n",
                       socket->fd, enabled != 0 ? 1 : 0);
        }
        *android_errno = 0;
        return 0;
      }
      if (request->option_output == nullptr ||
          request->option_output_capacity < sizeof(int32_t)) {
        *android_errno = 22;
        return -1;
      }
      const int32_t enabled =
          socket->pass_credentials.load(std::memory_order_acquire) ? 1 : 0;
      std::memcpy(request->option_output, &enabled, sizeof(enabled));
      *request->option_output_length = sizeof(enabled);
      *android_errno = 0;
      return 0;
    }
    if (request->operation == DARWIN_ART_FD_SOCKET_GETSOCKOPT &&
        request->level == IPPROTO_TCP && request->option == 11) {
      if (request->option_output == nullptr ||
          request->option_output_length == nullptr ||
          request->option_output_capacity == 0) {
        *android_errno = 22;
        return -1;
      }
      tcp_connection_info host{};
      socklen_t host_length = sizeof(host);
      if (getsockopt(socket->fd, IPPROTO_TCP, TCP_CONNECTION_INFO, &host,
                     &host_length) != 0) {
        *android_errno = AndroidErrno(errno);
        return -1;
      }
      AndroidTcpInfo android{};
      android.header[0] = host.tcpi_state;
      android.header[5] = static_cast<uint8_t>(host.tcpi_options & 0x0f);
      android.header[6] = static_cast<uint8_t>(
          (host.tcpi_snd_wscale & 0x0f) | ((host.tcpi_rcv_wscale & 0x0f) << 4));
      android.metrics[0] = host.tcpi_rto * 1000;
      android.metrics[2] = host.tcpi_maxseg;
      android.metrics[15] = host.tcpi_srtt * 1000;
      android.metrics[16] = host.tcpi_rttvar * 1000;
      android.metrics[17] = host.tcpi_snd_ssthresh;
      android.metrics[18] =
          host.tcpi_maxseg == 0 ? 0 : host.tcpi_snd_cwnd / host.tcpi_maxseg;
      android.rates[2] = host.tcpi_txbytes;
      android.rates[3] = host.tcpi_rxbytes;
      android.segments[0] = static_cast<uint32_t>(host.tcpi_txpackets);
      android.segments[1] = static_cast<uint32_t>(host.tcpi_rxpackets);
      android.byte_totals[0] = host.tcpi_txbytes;
      android.byte_totals[1] = host.tcpi_txretransmitbytes;
      android.tail[3] = host.tcpi_snd_wnd;
      android.tail[4] = host.tcpi_rcv_wnd;
      const size_t copied =
          std::min<size_t>(request->option_output_capacity, sizeof(android));
      std::memcpy(request->option_output, &android, copied);
      *request->option_output_length = static_cast<uint32_t>(copied);
      *android_errno = 0;
      return 0;
    }
    int host_level = 0;
    int host_option = 0;
    if (!TranslateOption(request->level, request->option, &host_level,
                         &host_option)) {
      if (SocketDebugEnabled()) {
        std::fprintf(stderr,
                     "DARWIN socket unsupported option host_fd=%d operation=%u "
                     "level=%d option=%d\n",
                     socket->fd, request->operation, request->level,
                     request->option);
      }
      *android_errno = 92;
      return -1;
    }
    if (request->operation == DARWIN_ART_FD_SOCKET_SETSOCKOPT) {
      struct timeval host_timeval{};
      const void *option_input = request->option_input;
      uint32_t option_input_length = request->option_input_length;
      if ((request->level == kAndroidSolSocket &&
           (request->option == kAndroidSoRcvtimeo ||
            request->option == kAndroidSoSndtimeo))) {
        if (request->option_input == nullptr ||
            request->option_input_length < sizeof(AndroidTimeval)) {
          *android_errno = 22;
          return -1;
        }
        AndroidTimeval android_timeval{};
        std::memcpy(&android_timeval, request->option_input,
                    sizeof(android_timeval));
        if (android_timeval.seconds < 0 || android_timeval.microseconds < 0 ||
            android_timeval.microseconds >= 1000000 ||
            android_timeval.seconds >
                static_cast<int64_t>(std::numeric_limits<time_t>::max())) {
          *android_errno = 22;
          return -1;
        }
        host_timeval.tv_sec = static_cast<time_t>(android_timeval.seconds);
        host_timeval.tv_usec =
            static_cast<suseconds_t>(android_timeval.microseconds);
        option_input = &host_timeval;
        option_input_length = sizeof(host_timeval);
      }
      const int result = setsockopt(socket->fd, host_level, host_option,
                                    option_input, option_input_length);
      if (SocketDebugEnabled()) {
        std::fprintf(stderr,
                     "DARWIN socket setsockopt host_fd=%d android_level=%d "
                     "android_option=%d host_level=%d host_option=%d length=%u "
                     "result=%d host_errno=%d\n",
                     socket->fd, request->level, request->option, host_level,
                     host_option, request->option_input_length, result,
                     result < 0 ? errno : 0);
      }
      *android_errno = result < 0 ? AndroidErrno(errno) : 0;
      return result;
    }
    socklen_t length = request->option_output_capacity;
    int result = 0;
    if (request->level == kAndroidSolSocket &&
        request->option == kAndroidSoError) {
      int host_error = 0;
      length = sizeof(host_error);
      result =
          getsockopt(socket->fd, host_level, host_option, &host_error, &length);
      if (result == 0) {
        const int32_t android_error = AndroidErrno(host_error);
        if (request->option_output_capacity < sizeof(android_error)) {
          *android_errno = 22;
          return -1;
        }
        std::memcpy(request->option_output, &android_error,
                    sizeof(android_error));
        length = sizeof(android_error);
      }
    } else if (request->level == kAndroidSolSocket &&
               (request->option == kAndroidSoReuseaddr ||
                request->option == kAndroidSoKeepalive)) {
      int host_value = 0;
      length = sizeof(host_value);
      result =
          getsockopt(socket->fd, host_level, host_option, &host_value, &length);
      if (result == 0) {
        const int32_t android_value = host_value == 0 ? 0 : 1;
        if (request->option_output_capacity < sizeof(android_value)) {
          *android_errno = 22;
          return -1;
        }
        std::memcpy(request->option_output, &android_value,
                    sizeof(android_value));
        length = sizeof(android_value);
      }
    } else if (request->level == kAndroidSolSocket &&
               (request->option == kAndroidSoRcvtimeo ||
                request->option == kAndroidSoSndtimeo)) {
      struct timeval host_timeval{};
      length = sizeof(host_timeval);
      result = getsockopt(socket->fd, host_level, host_option, &host_timeval,
                          &length);
      if (result == 0) {
        if (request->option_output_capacity < sizeof(AndroidTimeval) ||
            host_timeval.tv_usec < 0 || host_timeval.tv_usec >= 1000000) {
          *android_errno = 22;
          return -1;
        }
        AndroidTimeval android_timeval{
            static_cast<int64_t>(host_timeval.tv_sec),
            static_cast<int64_t>(host_timeval.tv_usec)};
        std::memcpy(request->option_output, &android_timeval,
                    sizeof(android_timeval));
        length = sizeof(android_timeval);
      }
    } else {
      result = getsockopt(socket->fd, host_level, host_option,
                          request->option_output, &length);
    }
    if (result == 0)
      *request->option_output_length = length;
    if (SocketDebugEnabled()) {
      int32_t output_value = 0;
      if (result == 0 && request->option_output != nullptr &&
          length >= sizeof(output_value)) {
        std::memcpy(&output_value, request->option_output,
                    sizeof(output_value));
      }
      std::fprintf(stderr,
                   "DARWIN socket getsockopt host_fd=%d android_level=%d "
                   "android_option=%d host_level=%d host_option=%d capacity=%u "
                   "length=%u result=%d host_errno=%d output_i32=%d\n",
                   socket->fd, request->level, request->option, host_level,
                   host_option, request->option_output_capacity,
                   static_cast<unsigned>(length), result,
                   result < 0 ? errno : 0, output_value);
    }
    *android_errno = result < 0 ? AndroidErrno(errno) : 0;
    return result;
  }
  *android_errno = 38;
  return -1;
}

int BrokerFailure(DarwinArtFdBrokerStatus status) {
  if (status == DARWIN_ART_FD_BROKER_STALE ||
      status == DARWIN_ART_FD_BROKER_WRONG_OWNER ||
      status == DARWIN_ART_FD_BROKER_WRONG_KIND)
    return 9;
  if (status == DARWIN_ART_FD_BROKER_EXHAUSTED)
    return 24;
  if (status == DARWIN_ART_FD_BROKER_UNSUPPORTED)
    return 38;
  return 22;
}

int OpenBinderDevice(int flags, uint32_t mode, int *android_errno) {
  constexpr int kAndroidAccessMode = 3;
  constexpr int kAndroidReadWrite = 2;
  constexpr int kAndroidOpenNonblock = 0x800;
  constexpr int kAndroidOpenCloexec = 0x80000;
  if (android_errno == nullptr)
    return -1;
  *android_errno = 0;
  if ((flags & kAndroidAccessMode) != kAndroidReadWrite || mode != 0 ||
      (flags & ~(kAndroidAccessMode | kAndroidOpenNonblock |
                 kAndroidOpenCloexec)) != 0) {
    *android_errno = 22;
    return -1;
  }
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr) {
    *android_errno = 19;
    return -1;
  }
  DarwinArtFdOwnerHandle owner = 0;
  {
    std::lock_guard lock(process->mutex);
    owner = process->binder_owner;
  }
  if (owner == 0) {
    *android_errno = 19;
    return -1;
  }
  int descriptor = -1;
  const auto status = darwin_art_fd_broker_publish_with_flags(
      process->broker, owner, 1,
      (flags & kAndroidOpenNonblock) != 0 ? DARWIN_ART_FD_STATUS_NONBLOCK : 0,
      (flags & kAndroidOpenCloexec) != 0 ? DARWIN_ART_FD_CLOEXEC : 0,
      &descriptor);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    *android_errno = BrokerFailure(status);
    return -1;
  }
  return descriptor;
}

DarwinArtFdSocketRequestV1 Request(uint32_t operation) {
  DarwinArtFdSocketRequestV1 request{};
  request.abi_version = DARWIN_ART_FD_SOCKET_REQUEST_ABI_V1;
  request.struct_size = sizeof(request);
  request.operation = operation;
  return request;
}

} // namespace

extern "C" int darwin_art_bionic_binder_fd_acquire_process(
    void **process_cookie, DarwinArtFdBroker **broker) {
  if (process_cookie == nullptr || broker == nullptr)
    return -1;
  *process_cookie = nullptr;
  *broker = nullptr;
  Process *process = AcquireProcess();
  if (process == nullptr || process->broker == nullptr) {
    if (process != nullptr)
      ReleaseProcess(process);
    return -1;
  }
  *process_cookie = process;
  *broker = process->broker;
  return 0;
}

extern "C" void darwin_art_bionic_binder_fd_release_process(
    void *process_cookie) {
  if (process_cookie != nullptr)
    ReleaseProcess(static_cast<Process *>(process_cookie));
}

extern "C" int darwin_art_bionic_socket_broker_install_unix_endpoint(
    const void *address, uint32_t length, const char *host_path) {
  PreserveErrno preserve;
  ProcessLease lease;
  if (lease.get() == nullptr)
    return Fail(38, -1);
  if (host_path == nullptr)
    return Fail(22, -1);
  const auto parsed = darwin_art::socket::ParseUnixAddress(address, length);
  if (!parsed || !lease.get()->unix_endpoints.Install(*parsed, host_path))
    return Fail(22, -1);
  return 0;
}

extern "C" int darwin_art_bionic_socket_broker_activate() {
  PreserveErrno preserve;
  darwin_art_fdsan_initialize();
  std::lock_guard global(g_process_mutex);
  if (g_process != nullptr)
    return -1;
  auto *process = new (std::nothrow) Process();
  if (process == nullptr)
    return -1;
  process->broker = darwin_art_fd_broker_create();
  if (process->broker == nullptr) {
    delete process;
    return -1;
  }
  DarwinArtFdOwnerV1 callbacks{DARWIN_ART_FD_OWNER_ABI_V7,
                               sizeof(DarwinArtFdOwnerV1),
                               process,
                               &OwnerRead,
                               &OwnerWrite,
                               &OwnerPoll,
                               &OwnerIoctl,
                               &OwnerClose,
                               nullptr,
                               nullptr,
                               &OwnerSocketOperation,
                               &OwnerPollMany,
                               &OwnerSetStatusFlags,
                               &OwnerCreatePollWake,
                               &OwnerSignalPollWake,
                               &OwnerClosePollWake,
                               &OwnerPollManyWithWake,
                               &OwnerExportHostFd};
  if (darwin_art_fd_broker_install_owner(process->broker, DARWIN_ART_FD_SOCKET,
                                         &callbacks, &process->socket_owner) !=
      DARWIN_ART_FD_BROKER_OK) {
    (void)darwin_art_fd_broker_destroy(process->broker);
    delete process;
    return -1;
  }
  DarwinArtFdOwnerV1 pipe_callbacks{DARWIN_ART_FD_OWNER_ABI_V7,
                                    sizeof(DarwinArtFdOwnerV1),
                                    process,
                                    &PipeOwnerRead,
                                    &PipeOwnerWrite,
                                    &OwnerPoll,
                                    &OwnerIoctl,
                                    &OwnerClose,
                                    nullptr,
                                    nullptr,
                                    nullptr,
                                    &OwnerPollMany,
                                    &OwnerSetStatusFlags,
                                    &OwnerCreatePollWake,
                                    &OwnerSignalPollWake,
                                    &OwnerClosePollWake,
                                    &OwnerPollManyWithWake,
                                    &OwnerExportHostFd};
  if (darwin_art_fd_broker_install_owner(
          process->broker, DARWIN_ART_FD_PIPE, &pipe_callbacks,
          &process->pipe_owner) != DARWIN_ART_FD_BROKER_OK) {
    (void)darwin_art_fd_broker_uninstall_owner(process->broker,
                                               process->socket_owner);
    (void)darwin_art_fd_broker_destroy(process->broker);
    delete process;
    return -1;
  }
  g_process = process;
  return 0;
}

extern "C" int darwin_art_bionic_socket_broker_deactivate() {
  PreserveErrno preserve;
  Process *process = nullptr;
  {
    std::lock_guard global(g_process_mutex);
    process = g_process;
    if (process == nullptr)
      return -1;
    std::unique_lock local(process->mutex);
    process->draining = true;
    g_process = nullptr;
    process->changed.wait(local, [&] { return process->active == 0; });
    if (process->binder_owner != 0 ||
        process->objects.load(std::memory_order_acquire) != 0 ||
        process->dns_results.load(std::memory_order_acquire) != 0 ||
        process->async_dns_queries.load(std::memory_order_acquire) != 0) {
      process->draining = false;
      g_process = process;
      return -1;
    }
  }
  if (darwin_art_fd_broker_flush_deferred_closes(process->broker) !=
      DARWIN_ART_FD_BROKER_OK) {
    std::abort();
  }
  darwin_art_bionic_dns_reset_for_test();
  if (process->objects.load(std::memory_order_acquire) != 0 ||
      process->dns_results.load(std::memory_order_acquire) != 0 ||
      process->async_dns_queries.load(std::memory_order_acquire) != 0) {
    std::lock_guard global(g_process_mutex);
    std::lock_guard local(process->mutex);
    process->draining = false;
    g_process = process;
    return -1;
  }
  if (darwin_art_fd_broker_wait_owner_quiescent(
          process->broker, process->pipe_owner) != DARWIN_ART_FD_BROKER_OK ||
      darwin_art_fd_broker_wait_owner_quiescent(
          process->broker, process->socket_owner) != DARWIN_ART_FD_BROKER_OK) {
    std::abort();
  }
  if (darwin_art_fd_broker_uninstall_owner(
          process->broker, process->pipe_owner) != DARWIN_ART_FD_BROKER_OK ||
      darwin_art_fd_broker_uninstall_owner(
          process->broker, process->socket_owner) != DARWIN_ART_FD_BROKER_OK ||
      darwin_art_fd_broker_destroy(process->broker) !=
          DARWIN_ART_FD_BROKER_OK) {
    std::abort();
  }
  delete process;
  return 0;
}

extern "C" int darwin_art_bionic_socket_broker_socket(int domain, int type,
                                                      int protocol) {
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  int host_domain = 0;
  int host_type = 0;
  int host_protocol = 0;
  bool nonblocking = false;
  if (!TranslateDomain(domain, &host_domain))
    return Fail(97, -1);
  if (!TranslateType(type, &host_type, &nonblocking))
    return Fail(94, -1);
  // Darwin has no connection-oriented Unix seqpacket. Do not silently create
  // a datagram endpoint for socket(); the separate socketpair path owns its
  // record-channel compatibility handling.
  if (domain == 1 && (type & 0xf) == kAndroidSockSeqPacket)
    return Fail(94, -1);
  if (!TranslateProtocol(protocol, &host_protocol))
    return Fail(93, -1);
  const int fd = native_fd::CreateCloseOnExecSocket(host_domain, host_type, host_protocol);
  if (fd < 0)
    return Fail(AndroidErrno(errno), -1);
  if (fcntl(fd, F_SETFD, FD_CLOEXEC) < 0 ||
      (nonblocking &&
       (fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK) < 0))) {
    const int error = AndroidErrno(errno);
    (void)close(fd);
    return Fail(error, -1);
  }
#ifdef SO_NOSIGPIPE
  int one = 1;
  if (setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof(one)) < 0) {
    const int error = AndroidErrno(errno);
    (void)close(fd);
    return Fail(error, -1);
  }
#endif
  auto *object = new (std::nothrow) HostFdObject{fd};
  if (object == nullptr) {
    (void)close(fd);
    return Fail(12, -1);
  }
  process->objects.fetch_add(1, std::memory_order_release);
  int guest_fd = -1;
  const int status_flags = nonblocking ? DARWIN_ART_FD_STATUS_NONBLOCK : 0;
  const int descriptor_flags =
      (type & kAndroidSockCloexec) != 0 ? DARWIN_ART_FD_CLOEXEC : 0;
  const DarwinArtFdBrokerStatus status =
      darwin_art_fd_broker_publish_with_flags(
          process->broker, process->socket_owner,
          reinterpret_cast<uint64_t>(object), status_flags, descriptor_flags,
          &guest_fd);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    int ignored = 0;
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(object), &ignored);
    return Fail(BrokerFailure(status), -1);
  }
  if (SocketDebugEnabled()) {
    std::fprintf(stderr,
                 "DARWIN socket create guest_fd=%d host_fd=%d domain=%d "
                 "type=0x%x protocol=%d\n",
                 guest_fd, fd, domain, type, protocol);
  }
  return guest_fd;
}

extern "C" int darwin_art_bionic_socket_broker_pipe2(int32_t descriptors[2],
                                                     int flags) {
  PreserveErrno preserve;
  if (descriptors == nullptr)
    return Fail(14, -1);
  constexpr int kAllowed = kAndroidONonblock | 02000000;
  if ((flags & ~kAllowed) != 0)
    return Fail(22, -1);
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);

  int host[2] = {-1, -1};
  if (native_fd::CreateCloseOnExecPipe(host) != 0)
    return Fail(AndroidErrno(errno), -1);
  if (fcntl(host[0], F_SETFD, FD_CLOEXEC) != 0 ||
      fcntl(host[1], F_SETFD, FD_CLOEXEC) != 0 ||
      ((flags & kAndroidONonblock) != 0 &&
       (fcntl(host[0], F_SETFL, fcntl(host[0], F_GETFL) | O_NONBLOCK) != 0 ||
        fcntl(host[1], F_SETFL, fcntl(host[1], F_GETFL) | O_NONBLOCK) != 0))) {
    const int error = AndroidErrno(errno);
    (void)close(host[0]);
    (void)close(host[1]);
    return Fail(error, -1);
  }

  auto *read_end = new (std::nothrow) HostFdObject{host[0]};
  auto *write_end = new (std::nothrow) HostFdObject{host[1]};
  if (read_end == nullptr || write_end == nullptr) {
    delete read_end;
    delete write_end;
    (void)close(host[0]);
    (void)close(host[1]);
    return Fail(12, -1);
  }
  process->objects.fetch_add(2, std::memory_order_release);

  int guest[2] = {-1, -1};
  const int status_flags =
      (flags & kAndroidONonblock) != 0 ? DARWIN_ART_FD_STATUS_NONBLOCK : 0;
  const int descriptor_flags =
      (flags & 02000000) != 0 ? DARWIN_ART_FD_CLOEXEC : 0;
  DarwinArtFdBrokerStatus status = darwin_art_fd_broker_publish_with_flags(
      process->broker, process->pipe_owner,
      reinterpret_cast<uint64_t>(read_end), status_flags, descriptor_flags,
      &guest[0]);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    int ignored = 0;
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(read_end), &ignored);
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(write_end), &ignored);
    return Fail(BrokerFailure(status), -1);
  }
  status = darwin_art_fd_broker_publish_with_flags(
      process->broker, process->pipe_owner,
      reinterpret_cast<uint64_t>(write_end), status_flags, descriptor_flags,
      &guest[1]);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    DarwinArtFdIoResult ignored_result{};
    (void)darwin_art_fd_broker_close_owned(process->broker, process->pipe_owner,
                                           guest[0], &ignored_result);
    int ignored_error = 0;
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(write_end),
                     &ignored_error);
    return Fail(BrokerFailure(status), -1);
  }
  descriptors[0] = guest[0];
  descriptors[1] = guest[1];
  return 0;
}

extern "C" int darwin_art_bionic_socket_broker_pipe(int32_t descriptors[2]) {
  return darwin_art_bionic_socket_broker_pipe2(descriptors, 0);
}

extern "C" int darwin_art_bionic_socket_broker_eventfd(uint32_t initial_value,
                                                       int flags) {
  PreserveErrno preserve;
  constexpr int kAllowed = kAndroidONonblock | 02000000 | 1;
  if ((flags & ~kAllowed) != 0)
    return Fail(22, -1);
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  int host[2] = {-1, -1};
  if (native_fd::CreateCloseOnExecSocketPair(AF_UNIX, SOCK_DGRAM, 0, host) != 0)
    return Fail(AndroidErrno(errno), -1);
  const bool nonblocking = (flags & kAndroidONonblock) != 0;
  if (fcntl(host[0], F_SETFD, FD_CLOEXEC) != 0 ||
      fcntl(host[1], F_SETFD, FD_CLOEXEC) != 0 ||
      (nonblocking &&
       (fcntl(host[0], F_SETFL, fcntl(host[0], F_GETFL) | O_NONBLOCK) != 0 ||
        fcntl(host[1], F_SETFL, fcntl(host[1], F_GETFL) | O_NONBLOCK) != 0))) {
    const int error = AndroidErrno(errno);
    (void)close(host[0]);
    (void)close(host[1]);
    return Fail(error, -1);
  }
  auto event = darwin_art::eventfd::Create(initial_value, (flags & 1) != 0,
                                           host[1]);
  if (event == nullptr) {
    (void)close(host[0]);
    (void)close(host[1]);
    return Fail(12, -1);
  }
  auto *object = new (std::nothrow)
      HostFdObject{host[0], host[1], nullptr, nullptr, event};
  if (object == nullptr) {
    (void)close(host[0]);
    (void)close(host[1]);
    return Fail(12, -1);
  }
  int signal_errno = 0;
  if (darwin_art::eventfd::EnsureSignaled(event, &signal_errno) != 0) {
    delete object;
    (void)close(host[0]);
    (void)close(host[1]);
    return Fail(AndroidErrno(signal_errno), -1);
  }
  process->objects.fetch_add(1, std::memory_order_release);
  int guest_fd = -1;
  const auto status = darwin_art_fd_broker_publish_with_flags(
      process->broker, process->pipe_owner, reinterpret_cast<uint64_t>(object),
      nonblocking ? DARWIN_ART_FD_STATUS_NONBLOCK : 0,
      (flags & 02000000) != 0 ? DARWIN_ART_FD_CLOEXEC : 0, &guest_fd);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    int ignored = 0;
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(object), &ignored);
    return Fail(BrokerFailure(status), -1);
  }
  return guest_fd;
}

extern "C" int darwin_art_bionic_socket_broker_timerfd_create(int clock_id,
                                                              int flags) {
  PreserveErrno preserve;
  constexpr int kAllowed = kAndroidONonblock | 02000000;
  if (clock_id != 1 || (flags & ~kAllowed) != 0)
    return Fail(22, -1);
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  int host[2] = {-1, -1};
  if (native_fd::CreateCloseOnExecSocketPair(AF_UNIX, SOCK_DGRAM, 0, host) != 0)
    return Fail(AndroidErrno(errno), -1);
  const bool nonblocking = (flags & kAndroidONonblock) != 0;
  if (fcntl(host[0], F_SETFD, FD_CLOEXEC) != 0 ||
      fcntl(host[1], F_SETFD, FD_CLOEXEC) != 0 ||
      (nonblocking &&
       (fcntl(host[0], F_SETFL, fcntl(host[0], F_GETFL) | O_NONBLOCK) != 0 ||
        fcntl(host[1], F_SETFL, fcntl(host[1], F_GETFL) | O_NONBLOCK) != 0))) {
    const int error = AndroidErrno(errno);
    (void)close(host[0]);
    (void)close(host[1]);
    return Fail(error, -1);
  }
  auto timer = std::make_shared<TimerState>();
  timer->write_fd = host[1];
  auto *object = new (std::nothrow) HostFdObject{host[0], host[1], timer};
  if (object == nullptr) {
    (void)close(host[0]);
    (void)close(host[1]);
    return Fail(12, -1);
  }
  process->objects.fetch_add(1, std::memory_order_release);
  int guest_fd = -1;
  const auto status = darwin_art_fd_broker_publish_with_flags(
      process->broker, process->pipe_owner, reinterpret_cast<uint64_t>(object),
      nonblocking ? DARWIN_ART_FD_STATUS_NONBLOCK : 0,
      (flags & 02000000) != 0 ? DARWIN_ART_FD_CLOEXEC : 0, &guest_fd);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    int ignored = 0;
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(object), &ignored);
    return Fail(BrokerFailure(status), -1);
  }
  return guest_fd;
}

extern "C" int
darwin_art_bionic_socket_broker_timerfd_settime(int fd, int flags,
                                                const AndroidItimerspec *value,
                                                AndroidItimerspec *old_value) {
  PreserveErrno preserve;
  if (value == nullptr || old_value != nullptr || flags != 0)
    return Fail(22, -1);
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  DarwinArtFdIoResult result{};
  const auto status = darwin_art_fd_broker_ioctl(
      process->broker, fd, DARWIN_ART_FD_PIPE, kTimerFdSettimeRequest,
      const_cast<AndroidItimerspec *>(value), &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  if (result.value < 0)
    return Fail(result.android_errno, -1);
  return static_cast<int>(result.value);
}

// Linux only packs epoll_event on x86_64. Android arm64 follows the native
// AArch64 UAPI layout: 4 bytes of events, 4 bytes of padding, then 8 bytes of
// data. Keep this host-side mirror naturally aligned or data.ptr is shifted.
struct AndroidEpollEvent {
  uint32_t events;
  uint64_t data;
};

static_assert(offsetof(AndroidEpollEvent, data) == 8);
static_assert(sizeof(AndroidEpollEvent) == 16);

extern "C" int darwin_art_bionic_socket_broker_epoll_create1(int flags) {
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  int descriptor = -1;
  const auto status =
      darwin_art_fd_broker_epoll_create1(process->broker, flags, &descriptor);
  return status == DARWIN_ART_FD_BROKER_OK ? descriptor
                                           : Fail(BrokerFailure(status), -1);
}

extern "C" int darwin_art_bionic_socket_broker_epoll_create(int size) {
  if (size <= 0)
    return Fail(22, -1);
  return darwin_art_bionic_socket_broker_epoll_create1(0);
}

extern "C" int darwin_art_bionic_socket_broker_epoll_ctl(int epoll_fd,
                                                         int operation,
                                                         int target_fd,
                                                         const void *event) {
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  DarwinArtFdEpollEvent translated{};
  const DarwinArtFdEpollEvent *translated_pointer = nullptr;
  if (event != nullptr) {
    AndroidEpollEvent android{};
    std::memcpy(&android, event, sizeof(android));
    translated.events = android.events;
    translated.data = android.data;
    translated_pointer = &translated;
  }
  DarwinArtFdIoResult result{};
  const auto status =
      darwin_art_fd_broker_epoll_ctl(process->broker, epoll_fd, operation,
                                     target_fd, translated_pointer, &result);
  if (SocketDebugEnabled()) {
    std::fprintf(stderr,
                 "DARWIN epoll ctl epoll_fd=%d operation=%d target_fd=%d "
                 "events=0x%x data=0x%llx status=%d result=%lld errno=%d\n",
                 epoll_fd, operation, target_fd, translated.events,
                 static_cast<unsigned long long>(translated.data),
                 static_cast<int>(status), static_cast<long long>(result.value),
                 result.android_errno);
  }
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  return result.value < 0 ? Fail(result.android_errno, -1) : 0;
}

extern "C" int darwin_art_bionic_socket_broker_epoll_wait(int epoll_fd,
                                                          void *events,
                                                          int capacity,
                                                          int timeout_ms) {
  PreserveErrno preserve;
  if (events == nullptr || capacity <= 0)
    return Fail(22, -1);
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  std::vector<DarwinArtFdEpollEvent> translated(static_cast<size_t>(capacity));
  DarwinArtFdIoResult result{};
  const auto status = darwin_art_fd_broker_epoll_wait(
      process->broker, epoll_fd, translated.data(), translated.size(),
      timeout_ms, &result);
  if (SocketDebugEnabled() &&
      (status != DARWIN_ART_FD_BROKER_OK || result.value != 0)) {
    std::fprintf(stderr,
                 "DARWIN epoll wait epoll_fd=%d capacity=%d timeout_ms=%d "
                 "status=%d result=%lld errno=%d\n",
                 epoll_fd, capacity, timeout_ms, static_cast<int>(status),
                 static_cast<long long>(result.value), result.android_errno);
  }
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  if (result.value < 0)
    return Fail(result.android_errno, -1);
  for (intptr_t index = 0; index < result.value; ++index) {
    const AndroidEpollEvent android{translated[index].events,
                                    translated[index].data};
    std::memcpy(static_cast<uint8_t *>(events) + index * sizeof(android),
                &android, sizeof(android));
  }
  return static_cast<int>(result.value);
}

extern "C" intptr_t
darwin_art_bionic_socket_broker_readv(int fd, const void *vectors, int count) {
  PreserveErrno preserve;
  if (count < 0 || count > 1024 || (count != 0 && vectors == nullptr))
    return Fail(22, intptr_t{-1});
  const auto *iov = static_cast<const AndroidIovec *>(vectors);
  intptr_t total = 0;
  for (int index = 0; index < count; ++index) {
    const intptr_t result = darwin_art_bionic_socket_broker_read(
        fd, iov[index].base, iov[index].length);
    if (result < 0)
      return total == 0 ? -1 : total;
    total += result;
    if (static_cast<size_t>(result) != iov[index].length)
      break;
  }
  return total;
}

extern "C" intptr_t
darwin_art_bionic_socket_broker_writev(int fd, const void *vectors, int count) {
  PreserveErrno preserve;
  if (count < 0 || count > 1024 || (count != 0 && vectors == nullptr))
    return Fail(22, intptr_t{-1});
  const auto *iov = static_cast<const AndroidIovec *>(vectors);
  intptr_t total = 0;
  for (int index = 0; index < count; ++index) {
    const intptr_t result = darwin_art_bionic_socket_broker_write(
        fd, iov[index].base, iov[index].length);
    if (result < 0)
      return total == 0 ? -1 : total;
    total += result;
    if (static_cast<size_t>(result) != iov[index].length)
      break;
  }
  return total;
}

extern "C" int
darwin_art_bionic_socket_broker_socketpair(int domain, int type, int protocol,
                                           int32_t descriptors[2]) {
  PreserveErrno preserve;
  if (descriptors == nullptr)
    return Fail(14, -1);
  if (domain != 1 || protocol != 0)
    return Fail(domain != 1 ? 97 : 93, -1);
  int host_type = 0;
  bool nonblocking = false;
  if (!TranslateType(type, &host_type, &nonblocking))
    return Fail(94, -1);
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  int host[2] = {-1, -1};
  if (native_fd::CreateCloseOnExecSocketPair(AF_UNIX, host_type, 0, host) != 0)
    return Fail(AndroidErrno(errno), -1);
#ifdef SO_NOSIGPIPE
  // socket() applies this policy to every broker-owned socket. Keep
  // socketpair endpoints identical: MSG_NOSIGNAL is translated by the
  // broker, but callers are also allowed to use a plain send() on an
  // InputChannel descriptor. A broken peer must report EPIPE, never deliver
  // SIGPIPE to the ART process.
  int no_sigpipe = 1;
  for (int index = 0; index < 2; ++index) {
    if (setsockopt(host[index], SOL_SOCKET, SO_NOSIGPIPE, &no_sigpipe,
                   sizeof(no_sigpipe)) < 0) {
      const int error = AndroidErrno(errno);
      (void)close(host[0]);
      (void)close(host[1]);
      return Fail(error, -1);
    }
  }
#endif
  for (int index = 0; index < 2; ++index) {
    if (fcntl(host[index], F_SETFD, FD_CLOEXEC) != 0 ||
        (nonblocking && fcntl(host[index], F_SETFL,
                              fcntl(host[index], F_GETFL) | O_NONBLOCK) != 0)) {
      const int error = AndroidErrno(errno);
      (void)close(host[0]);
      (void)close(host[1]);
      return Fail(error, -1);
    }
  }
  auto *first = new (std::nothrow) HostFdObject{host[0]};
  auto *second = new (std::nothrow) HostFdObject{host[1]};
  if (first == nullptr || second == nullptr) {
    delete first;
    delete second;
    (void)close(host[0]);
    (void)close(host[1]);
    return Fail(12, -1);
  }
  process->objects.fetch_add(2, std::memory_order_release);
  const int status_flags = nonblocking ? DARWIN_ART_FD_STATUS_NONBLOCK : 0;
  const int descriptor_flags =
      (type & kAndroidSockCloexec) != 0 ? DARWIN_ART_FD_CLOEXEC : 0;
  int guest[2] = {-1, -1};
  auto status = darwin_art_fd_broker_publish_with_flags(
      process->broker, process->socket_owner, reinterpret_cast<uint64_t>(first),
      status_flags, descriptor_flags, &guest[0]);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    int ignored = 0;
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(first), &ignored);
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(second), &ignored);
    return Fail(BrokerFailure(status), -1);
  }
  status = darwin_art_fd_broker_publish_with_flags(
      process->broker, process->socket_owner,
      reinterpret_cast<uint64_t>(second), status_flags, descriptor_flags,
      &guest[1]);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    DarwinArtFdIoResult ignored_result{};
    (void)darwin_art_fd_broker_close_owned(
        process->broker, process->socket_owner, guest[0], &ignored_result);
    int ignored = 0;
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(second), &ignored);
    return Fail(BrokerFailure(status), -1);
  }
  descriptors[0] = guest[0];
  descriptors[1] = guest[1];
  return 0;
}

extern "C" intptr_t darwin_art_bionic_socket_broker_message_unsupported() {
  PreserveErrno preserve;
  if (SocketDebugEnabled()) {
    std::fprintf(stderr, "DARWIN socket sendmsg/recvmsg unsupported\n");
  }
  return Fail(95, intptr_t{-1});
}

extern "C" int darwin_art_bionic_socket_broker_dup(int fd) {
  PreserveErrno preserve;
  if ((static_cast<uint32_t>(fd) & kCentralBrokerTokenTopMask) !=
      kCentralBrokerTokenMarker) {
    const int result = darwin_art_android_shared_memory_dup(fd);
    if (result != -2)
      return result >= 0 ? result : Fail(5, -1);
    // Regular/private files live in the filesystem facade's guest table, not
    // the central socket/pipe table.  ParcelFileDescriptor and Binder must be
    // able to duplicate either kind through this process-wide FD entrypoint.
    const int filesystem_duplicate =
        darwin_art_bionic_fs_fcntl_core(fd, kAndroidFDupfd, 0);
    if (filesystem_duplicate >= 0)
      return filesystem_duplicate;
  }
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  int duplicate = -1;
  const auto status = darwin_art_fd_broker_dup(process->broker, fd, &duplicate);
  return status == DARWIN_ART_FD_BROKER_OK ? duplicate
                                           : Fail(BrokerFailure(status), -1);
}

extern "C" int darwin_art_bionic_socket_broker_dup2(int old_fd, int new_fd) {
  PreserveErrno preserve;
  if ((static_cast<uint32_t>(old_fd) & kCentralBrokerTokenTopMask) !=
          kCentralBrokerTokenMarker ||
      (static_cast<uint32_t>(new_fd) & kCentralBrokerTokenTopMask) !=
          kCentralBrokerTokenMarker) {
    // The libcore socket path only supplies central-broker descriptors. Do not
    // pass a guest token to the host dup2 implementation or reinterpret a
    // private filesystem descriptor as a host fd.
    return Fail(9, -1);
  }
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  DarwinArtFdIoResult result{};
  const auto status =
      darwin_art_fd_broker_dup2(process->broker, old_fd, new_fd, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  return result.value < 0 ? Fail(result.android_errno, -1)
                          : static_cast<int>(result.value);
}

extern "C" int darwin_art_bionic_socket_broker_unsupported_int() {
  PreserveErrno preserve;
  return Fail(38, -1);
}

extern "C" intptr_t darwin_art_bionic_socket_broker_read(int fd, void *bytes,
                                                         size_t count) {
  if ((static_cast<uint32_t>(fd) & kCentralBrokerTokenTopMask) !=
      kCentralBrokerTokenMarker)
    return darwin_art_bionic_fs_read_core(fd, bytes, count);
  PreserveErrno preserve;
  if (bytes == nullptr && count != 0)
    return Fail(14, intptr_t{-1});
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(9, intptr_t{-1});
  DarwinArtFdIoResult result{};
  const DarwinArtFdBrokerStatus status =
      darwin_art_fd_broker_read(process->broker, fd, bytes, count, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), intptr_t{-1});
  return result.value < 0 ? Fail(result.android_errno, intptr_t{-1})
                          : result.value;
}

extern "C" intptr_t
darwin_art_bionic_socket_broker_write(int fd, const void *bytes, size_t count) {
  if ((static_cast<uint32_t>(fd) & kCentralBrokerTokenTopMask) !=
      kCentralBrokerTokenMarker)
    return darwin_art_bionic_fs_write_core(fd, bytes, count);
  PreserveErrno preserve;
  if (bytes == nullptr && count != 0)
    return Fail(14, intptr_t{-1});
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(9, intptr_t{-1});
  DarwinArtFdIoResult result{};
  const DarwinArtFdBrokerStatus status =
      darwin_art_fd_broker_write(process->broker, fd, bytes, count, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), intptr_t{-1});
  return result.value < 0 ? Fail(result.android_errno, intptr_t{-1})
                          : result.value;
}

extern "C" intptr_t
darwin_art_bionic_socket_broker___read_chk(int fd, void *bytes, size_t count,
                                           size_t buffer_size) {
  if (SocketDebugEnabled() && ShouldLogSocketReadChk()) {
    std::fprintf(stderr,
                 "DARWIN socket read_chk op=__read_chk fd=%d count=%zu "
                 "buffer_size=%zu\n",
                 fd, count, buffer_size);
  }
  if (count > buffer_size)
    return Fail(34, intptr_t{-1});
  return darwin_art_bionic_socket_broker_read(fd, bytes, count);
}

extern "C" intptr_t
darwin_art_bionic_socket_broker___write_chk(int fd, const void *bytes,
                                            size_t count, size_t buffer_size) {
  if (count > buffer_size)
    return Fail(34, intptr_t{-1});
  return darwin_art_bionic_socket_broker_write(fd, bytes, count);
}

extern "C" int
darwin_art_bionic_socket_broker_poll(DarwinArtBionicPollFd *descriptors,
                                     size_t count, int timeout_ms) {
  static_assert(sizeof(DarwinArtBionicPollFd) == sizeof(DarwinArtFdPollEntry));
  static_assert(alignof(DarwinArtBionicPollFd) ==
                alignof(DarwinArtFdPollEntry));
  PreserveErrno preserve;
  if (timeout_ms < -1)
    return Fail(22, -1);
  if (count != 0 && descriptors == nullptr)
    return Fail(14, -1);
  if (count > 65536)
    return Fail(22, -1);
  if (count == 0) {
    const int value = poll(nullptr, 0, timeout_ms);
    return value < 0 ? Fail(AndroidErrno(errno), -1) : value;
  }
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  std::vector<DarwinArtFdPollEntry> entries(count);
  for (size_t index = 0; index < count; ++index) {
    entries[index] = DarwinArtFdPollEntry{descriptors[index].fd,
                                          descriptors[index].events, 0};
  }
  DarwinArtFdIoResult result{};
  const DarwinArtFdBrokerStatus status = darwin_art_fd_broker_poll_wait(
      process->broker, entries.data(), count, timeout_ms, &result);
  for (size_t index = 0; index < count; ++index)
    descriptors[index].revents = entries[index].revents;
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  return result.value < 0 ? Fail(result.android_errno, -1)
                          : static_cast<int>(result.value);
}

// Android arm64 uses a 1024-bit fd_set with 64-bit words.  The host Darwin
// fd_set has a different word layout, so do not cast these guest buffers to
// the host type.  Socket descriptors exposed by the central broker are
// virtual and cannot be passed to host select; translate each selected guest
// fd through the broker's poll contract instead.
struct AndroidSelectTimeval {
  int64_t seconds;
  int64_t microseconds;
};

static_assert(sizeof(AndroidSelectTimeval) == 16);

static uint64_t SelectFdSetWord(const void *set, size_t word) {
  uint64_t value = 0;
  std::memcpy(&value, static_cast<const uint8_t *>(set) + word * sizeof(value),
              sizeof(value));
  return value;
}

static void ClearSelectFdSet(void *set) {
  if (set != nullptr)
    std::memset(set, 0, kAndroidFdSetWords * sizeof(uint64_t));
}

static void SetSelectFd(void *set, int fd) {
  if (set == nullptr || fd < 0 || fd >= kAndroidFdSetBits)
    return;
  const size_t word = static_cast<size_t>(fd) / kAndroidFdSetWordBits;
  const uint64_t mask = UINT64_C(1)
                        << (static_cast<unsigned>(fd) % kAndroidFdSetWordBits);
  uint64_t value = SelectFdSetWord(set, word);
  value |= mask;
  std::memcpy(static_cast<uint8_t *>(set) + word * sizeof(value), &value,
              sizeof(value));
}

extern "C" int
darwin_art_bionic_socket_broker_select(int nfds, void *readfds, void *writefds,
                                       void *exceptfds,
                                       AndroidSelectTimeval *timeout) {
  PreserveErrno preserve;
  const bool trace = SocketDebugEnabled() && ShouldLogSocketSelect();
  size_t trace_descriptors = 0;
  auto trace_result = [&](const char *phase, int value, int error) {
    if (trace) {
      std::fprintf(stderr,
                   "DARWIN socket select op=select phase=%s nfds=%d "
                   "descriptors=%zu result=%d android_errno=%d\n",
                   phase, nfds, trace_descriptors, value, error);
    }
  };

  if (nfds < 0 || nfds > kAndroidFdSetBits) {
    if (trace)
      std::fprintf(stderr,
                   "DARWIN socket select op=select phase=invalid "
                   "nfds=%d result=-1 android_errno=22\n",
                   nfds);
    return Fail(22, -1);
  }

  uint64_t timeout_us = 0;
  if (timeout != nullptr) {
    AndroidSelectTimeval input{};
    std::memcpy(&input, timeout, sizeof(input));
    if (input.seconds < 0 || input.microseconds < 0 ||
        input.microseconds >= 1'000'000 ||
        input.seconds > (INT64_MAX - input.microseconds) / 1'000'000) {
      if (trace)
        std::fprintf(stderr,
                     "DARWIN socket select op=select phase=invalid-timeout "
                     "nfds=%d result=-1 android_errno=22\n",
                     nfds);
      return Fail(22, -1);
    }
    timeout_us = static_cast<uint64_t>(input.seconds) * 1'000'000u +
                 static_cast<uint64_t>(input.microseconds);
  }

  std::vector<DarwinArtFdPollEntry> entries;
  entries.reserve(static_cast<size_t>(nfds));
  for (int fd = 0; fd < nfds; ++fd) {
    const size_t word = static_cast<size_t>(fd) / kAndroidFdSetWordBits;
    const uint64_t mask =
        UINT64_C(1) << (static_cast<unsigned>(fd) % kAndroidFdSetWordBits);
    const bool readable =
        readfds != nullptr && (SelectFdSetWord(readfds, word) & mask) != 0;
    const bool writable =
        writefds != nullptr && (SelectFdSetWord(writefds, word) & mask) != 0;
    const bool exceptional =
        exceptfds != nullptr && (SelectFdSetWord(exceptfds, word) & mask) != 0;
    int16_t events = 0;
    if (readable)
      events |= POLLIN;
    if (writable)
      events |= POLLOUT;
    if (exceptional)
      events |= POLLPRI;
    if (events != 0)
      entries.push_back(DarwinArtFdPollEntry{fd, events, 0});
  }

  trace_descriptors = entries.size();
  if (trace) {
    std::fprintf(stderr,
                 "DARWIN socket select op=select phase=enter nfds=%d "
                 "descriptors=%zu timeout_us=%llu\n",
                 nfds, entries.size(),
                 static_cast<unsigned long long>(timeout_us));
  }

  const auto started = std::chrono::steady_clock::now();
  auto elapsed_us = [&]() -> uint64_t {
    const auto elapsed = std::chrono::duration_cast<std::chrono::microseconds>(
        std::chrono::steady_clock::now() - started);
    return elapsed.count() <= 0 ? 0 : static_cast<uint64_t>(elapsed.count());
  };
  auto update_timeout = [&](uint64_t elapsed) {
    if (timeout == nullptr)
      return;
    const uint64_t remaining = elapsed >= timeout_us ? 0 : timeout_us - elapsed;
    const AndroidSelectTimeval output{
        static_cast<int64_t>(remaining / 1'000'000u),
        static_cast<int64_t>(remaining % 1'000'000u)};
    std::memcpy(timeout, &output, sizeof(output));
  };

  for (;;) {
    uint64_t remaining_us = UINT64_MAX;
    if (timeout != nullptr) {
      const uint64_t elapsed = elapsed_us();
      remaining_us = elapsed >= timeout_us ? 0 : timeout_us - elapsed;
    }
    const uint64_t rounded_ms = remaining_us == UINT64_MAX
                                    ? UINT64_MAX
                                    : (remaining_us + 999u) / 1'000u;
    const int timeout_ms =
        rounded_ms == UINT64_MAX
            ? -1
            : static_cast<int>(std::min<uint64_t>(rounded_ms, INT_MAX));

    DarwinArtFdIoResult result{};
    DarwinArtFdBrokerStatus status = DARWIN_ART_FD_BROKER_OK;
    if (entries.empty()) {
      const int value = ::poll(nullptr, 0, timeout_ms);
      const int saved = errno;
      result.value = value;
      result.android_errno = value < 0 ? AndroidErrno(saved) : 0;
    } else {
      ProcessLease lease;
      Process *process = lease.get();
      if (process == nullptr)
        return Fail(38, -1);
      status = darwin_art_fd_broker_poll_wait(
          process->broker, entries.data(), entries.size(), timeout_ms, &result);
    }
    if (status != DARWIN_ART_FD_BROKER_OK) {
      const int error = BrokerFailure(status);
      trace_result("broker-error", -1, error);
      return Fail(error, -1);
    }
    if (result.value < 0) {
      update_timeout(elapsed_us());
      trace_result("poll-error", -1, result.android_errno);
      return Fail(result.android_errno, -1);
    }

    bool invalid = false;
    for (const auto &entry : entries) {
      if ((entry.revents & POLLNVAL) != 0) {
        invalid = true;
        break;
      }
    }
    if (invalid) {
      trace_result("ebadf", -1, 9);
      return Fail(9, -1);
    }

    if (result.value == 0) {
      if (timeout == nullptr)
        continue;
      const uint64_t elapsed = elapsed_us();
      if (elapsed < timeout_us)
        continue;
      ClearSelectFdSet(readfds);
      ClearSelectFdSet(writefds);
      ClearSelectFdSet(exceptfds);
      update_timeout(elapsed);
      if (trace)
        std::fprintf(stderr,
                     "DARWIN socket select op=select phase=timeout "
                     "nfds=%d result=0 android_errno=0\n",
                     nfds);
      return 0;
    }

    ClearSelectFdSet(readfds);
    ClearSelectFdSet(writefds);
    ClearSelectFdSet(exceptfds);
    int ready = 0;
    for (const auto &entry : entries) {
      const int16_t revents = entry.revents;
      constexpr int16_t kPollInSet = POLLIN | POLLHUP | POLLERR |
                                     0x0040 /* POLLRDNORM */ |
                                     0x0080 /* POLLRDBAND */;
      constexpr int16_t kPollOutSet =
          POLLOUT | POLLERR | 0x0100 /* POLLWRNORM */ | 0x0200 /* POLLWRBAND */;
      const bool readable =
          (entry.events & POLLIN) != 0 && (revents & kPollInSet) != 0;
      const bool writable =
          (entry.events & POLLOUT) != 0 && (revents & kPollOutSet) != 0;
      const bool exceptional =
          (entry.events & POLLPRI) != 0 && (revents & POLLPRI) != 0;
      if (readable) {
        SetSelectFd(readfds, entry.fd);
        ++ready;
      }
      if (writable) {
        SetSelectFd(writefds, entry.fd);
        ++ready;
      }
      if (exceptional) {
        SetSelectFd(exceptfds, entry.fd);
        ++ready;
      }
    }
    update_timeout(elapsed_us());
    if (trace)
      std::fprintf(stderr,
                   "DARWIN socket select op=select phase=ready nfds=%d "
                   "descriptors=%zu result=%d android_errno=0\n",
                   nfds, entries.size(), ready);
    return ready;
  }
}

extern "C" int darwin_art_bionic_socket_broker_connect(int fd,
                                                       const void *address,
                                                       uint32_t length) {
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  DarwinArtFdSocketRequestV1 request = Request(DARWIN_ART_FD_SOCKET_CONNECT);
  request.address = address;
  request.address_length = length;
  DarwinArtFdIoResult result{};
  const DarwinArtFdBrokerStatus status = darwin_art_fd_broker_socket_operation(
      process->broker, process->socket_owner, fd, &request, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  if (result.value < 0)
    return Fail(result.android_errno, -1);
  return static_cast<int>(result.value);
}

static int SocketIntegerOperation(int fd, DarwinArtFdSocketRequestV1 *request) {
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  DarwinArtFdIoResult result{};
  const auto status = darwin_art_fd_broker_socket_operation(
      process->broker, process->socket_owner, fd, request, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  return result.value < 0 ? Fail(result.android_errno, -1)
                          : static_cast<int>(result.value);
}

extern "C" int darwin_art_bionic_socket_broker_bind(int fd, const void *address,
                                                    uint32_t length) {
  PreserveErrno preserve;
  auto request = Request(DARWIN_ART_FD_SOCKET_BIND);
  request.address = address;
  request.address_length = length;
  return SocketIntegerOperation(fd, &request);
}

extern "C" int darwin_art_bionic_socket_broker_listen(int fd, int backlog) {
  PreserveErrno preserve;
  auto request = Request(DARWIN_ART_FD_SOCKET_LISTEN);
  request.argument = backlog;
  return SocketIntegerOperation(fd, &request);
}

extern "C" int darwin_art_bionic_socket_broker_accept4(int fd, void *address,
                                                       uint32_t *length,
                                                       int flags) {
  PreserveErrno preserve;
  auto request = Request(DARWIN_ART_FD_SOCKET_ACCEPT4);
  request.flags = flags;
  request.output_address = address;
  request.output_address_capacity = length == nullptr ? 0 : *length;
  request.output_address_length = length;
  return SocketIntegerOperation(fd, &request);
}

extern "C" int darwin_art_bionic_socket_broker_accept(int fd, void *address,
                                                      uint32_t *length) {
  return darwin_art_bionic_socket_broker_accept4(fd, address, length, 0);
}

static int SocketName(int fd, void *address, uint32_t *length,
                      uint32_t operation) {
  if (address == nullptr || length == nullptr)
    return Fail(14, -1);
  auto request = Request(operation);
  request.output_address = address;
  request.output_address_capacity = *length;
  request.output_address_length = length;
  return SocketIntegerOperation(fd, &request);
}

extern "C" int darwin_art_bionic_socket_broker_getsockname(int fd,
                                                           void *address,
                                                           uint32_t *length) {
  PreserveErrno preserve;
  return SocketName(fd, address, length, DARWIN_ART_FD_SOCKET_GETSOCKNAME);
}

extern "C" int darwin_art_bionic_socket_broker_getpeername(int fd,
                                                           void *address,
                                                           uint32_t *length) {
  PreserveErrno preserve;
  return SocketName(fd, address, length, DARWIN_ART_FD_SOCKET_GETPEERNAME);
}

extern "C" intptr_t darwin_art_bionic_socket_broker_send(int fd,
                                                         const void *bytes,
                                                         size_t count,
                                                         int flags) {
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, intptr_t{-1});
  DarwinArtFdSocketRequestV1 request = Request(DARWIN_ART_FD_SOCKET_SEND);
  request.flags = flags;
  request.input_bytes = bytes;
  request.byte_count = count;
  DarwinArtFdIoResult result{};
  const DarwinArtFdBrokerStatus status = darwin_art_fd_broker_socket_operation(
      process->broker, process->socket_owner, fd, &request, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), intptr_t{-1});
  if (result.value < 0)
    return Fail(result.android_errno, intptr_t{-1});
  return result.value;
}

extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int fd, void *bytes,
                                                         size_t count,
                                                         int flags) {
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, intptr_t{-1});
  DarwinArtFdSocketRequestV1 request = Request(DARWIN_ART_FD_SOCKET_RECV);
  request.flags = flags;
  request.output_bytes = bytes;
  request.byte_count = count;
  DarwinArtFdIoResult result{};
  const DarwinArtFdBrokerStatus status = darwin_art_fd_broker_socket_operation(
      process->broker, process->socket_owner, fd, &request, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), intptr_t{-1});
  if (result.value < 0)
    return Fail(result.android_errno, intptr_t{-1});
  return result.value;
}

extern "C" intptr_t
darwin_art_bionic_socket_broker_sendto(int fd, const void *bytes, size_t count,
                                       int flags, const void *address,
                                       uint32_t address_length) {
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, intptr_t{-1});
  DarwinArtFdSocketRequestV1 request = Request(DARWIN_ART_FD_SOCKET_SENDTO);
  request.flags = flags;
  request.input_bytes = bytes;
  request.byte_count = count;
  request.address = address;
  request.address_length = address_length;
  DarwinArtFdIoResult result{};
  const auto status = darwin_art_fd_broker_socket_operation(
      process->broker, process->socket_owner, fd, &request, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), intptr_t{-1});
  return result.value < 0 ? Fail(result.android_errno, intptr_t{-1})
                          : result.value;
}

extern "C" intptr_t
darwin_art_bionic_socket_broker_recvfrom(int fd, void *bytes, size_t count,
                                         int flags, void *address,
                                         uint32_t *address_length) {
  PreserveErrno preserve;
  if (address != nullptr && address_length == nullptr)
    return Fail(14, intptr_t{-1});
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, intptr_t{-1});
  DarwinArtFdSocketRequestV1 request = Request(DARWIN_ART_FD_SOCKET_RECVFROM);
  request.flags = flags;
  request.output_bytes = bytes;
  request.byte_count = count;
  request.output_address = address;
  request.output_address_capacity =
      address_length == nullptr ? 0 : *address_length;
  request.output_address_length = address_length;
  DarwinArtFdIoResult result{};
  const auto status = darwin_art_fd_broker_socket_operation(
      process->broker, process->socket_owner, fd, &request, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), intptr_t{-1});
  return result.value < 0 ? Fail(result.android_errno, intptr_t{-1})
                          : result.value;
}

extern "C" int darwin_art_bionic_socket_broker_getsockopt(int fd, int level,
                                                          int option,
                                                          void *value,
                                                          uint32_t *length) {
  PreserveErrno preserve;
  if (value == nullptr || length == nullptr)
    return Fail(14, -1);
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  DarwinArtFdSocketRequestV1 request = Request(DARWIN_ART_FD_SOCKET_GETSOCKOPT);
  request.level = level;
  request.option = option;
  request.option_output = value;
  request.option_output_capacity = *length;
  request.option_output_length = length;
  DarwinArtFdIoResult result{};
  const auto status = darwin_art_fd_broker_socket_operation(
      process->broker, process->socket_owner, fd, &request, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  return result.value < 0 ? Fail(result.android_errno, -1)
                          : static_cast<int>(result.value);
}

extern "C" int darwin_art_bionic_socket_broker_setsockopt(int fd, int level,
                                                          int option,
                                                          const void *value,
                                                          uint32_t length) {
  PreserveErrno preserve;
  if (value == nullptr)
    return Fail(14, -1);
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  DarwinArtFdSocketRequestV1 request = Request(DARWIN_ART_FD_SOCKET_SETSOCKOPT);
  request.level = level;
  request.option = option;
  request.option_input = value;
  request.option_input_length = length;
  DarwinArtFdIoResult result{};
  const auto status = darwin_art_fd_broker_socket_operation(
      process->broker, process->socket_owner, fd, &request, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  return result.value < 0 ? Fail(result.android_errno, -1)
                          : static_cast<int>(result.value);
}

extern "C" int darwin_art_bionic_socket_broker_shutdown(int fd, int how) {
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  DarwinArtFdSocketRequestV1 request = Request(DARWIN_ART_FD_SOCKET_SHUTDOWN);
  request.argument = how;
  DarwinArtFdIoResult result{};
  const auto status = darwin_art_fd_broker_socket_operation(
      process->broker, process->socket_owner, fd, &request, &result);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  return result.value < 0 ? Fail(result.android_errno, -1)
                          : static_cast<int>(result.value);
}

extern "C" int darwin_art_bionic_socket_broker_close_unchecked(int fd) {
  const uint32_t token = static_cast<uint32_t>(fd);
  if ((token & kCentralBrokerTokenTopMask) != kCentralBrokerTokenMarker) {
    const int shared_result = darwin_art_android_shared_memory_close(fd);
    if (shared_result != 0)
      return shared_result > 0 ? 0 : Fail(5, -1);
    const int result = darwin_art_bionic_fs_close_core(fd);
    if (result < 0 && std::getenv("DARWIN_ART_DEBUG_CLOSE") != nullptr) {
      std::fprintf(stderr,
                   "DARWIN debug close: fd=%d class=fs-or-host result=%d\n", fd,
                   result);
    }
    return result;
  }
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(9, -1);
  DarwinArtFdIoResult result{};
  const DarwinArtFdBrokerStatus status =
      darwin_art_fd_broker_close(process->broker, fd, &result);
  if (status != DARWIN_ART_FD_BROKER_OK) {
    if (std::getenv("DARWIN_ART_DEBUG_CLOSE") != nullptr) {
      std::fprintf(stderr, "DARWIN debug close: fd=%d class=broker status=%d\n",
                   fd, static_cast<int>(status));
    }
    return Fail(BrokerFailure(status), -1);
  }
  if (result.value < 0)
    return Fail(result.android_errno, -1);
  return static_cast<int>(result.value);
}

extern "C" int darwin_art_bionic_socket_broker_fcntl(int fd, int command,
                                                     intptr_t argument) {
  PreserveErrno preserve;
  if ((static_cast<uint32_t>(fd) & kCentralBrokerTokenTopMask) !=
      kCentralBrokerTokenMarker) {
    int shared_result = -1;
    if (darwin_art_android_shared_memory_fcntl(fd, command, argument,
                                               &shared_result) != 0) {
      return shared_result >= 0 ? shared_result : Fail(22, -1);
    }
    return darwin_art_bionic_fs_fcntl_core(fd, command, argument);
  }
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  int value = 0;
  DarwinArtFdBrokerStatus status = DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  switch (command) {
  case kAndroidFDupfd:
    status = darwin_art_fd_broker_dup(process->broker, fd, &value);
    break;
  case kAndroidFDupfdCloexec:
    status = darwin_art_fd_broker_fcntl_dupfd_cloexec(
        process->broker, fd, static_cast<int>(argument), &value);
    break;
  case kAndroidFGetfd:
    status =
        darwin_art_fd_broker_get_descriptor_flags(process->broker, fd, &value);
    break;
  case kAndroidFSetfd:
    if ((argument & ~kAndroidFdCloexec) != 0)
      return Fail(22, -1);
    status = darwin_art_fd_broker_set_descriptor_flags(
        process->broker, fd, static_cast<int>(argument));
    break;
  case kAndroidFGetfl:
    status = darwin_art_fd_broker_get_status_flags(process->broker, fd, &value);
    if (status == DARWIN_ART_FD_BROKER_OK)
      value |= 2;
    break;
  case kAndroidFSetfl: {
    DarwinArtFdIoResult result{};
    status = darwin_art_fd_broker_set_status_flags_io(
        process->broker, fd,
        static_cast<int>(argument) & (kAndroidOAppend | kAndroidONonblock),
        &result);
    if (status == DARWIN_ART_FD_BROKER_OK && result.value < 0)
      return Fail(result.android_errno, -1);
  } break;
  default:
    return Fail(22, -1);
  }
  if (status != DARWIN_ART_FD_BROKER_OK)
    return Fail(BrokerFailure(status), -1);
  return command == kAndroidFSetfd || command == kAndroidFSetfl ? 0 : value;
}

extern "C" int darwin_art_bionic_fd_export_for_scm(int guest_fd) {
  PreserveErrno preserve;
  if ((static_cast<uint32_t>(guest_fd) & kCentralBrokerTokenTopMask) ==
      kCentralBrokerTokenMarker) {
    ProcessLease lease;
    Process *process = lease.get();
    if (process == nullptr)
      return Fail(9, -1);
    int host_fd = -1;
    DarwinArtFdIoResult result{};
    const auto status = darwin_art_fd_broker_export_host_fd(
        process->broker, guest_fd, &host_fd, &result);
    if (status != DARWIN_ART_FD_BROKER_OK || host_fd < 0)
      return Fail(result.android_errno != 0 ? result.android_errno
                                            : BrokerFailure(status),
                  -1);
    return host_fd;
  }
  int host_fd = -1;
  const int filesystem =
      darwin_art_bionic_fs_dup_host_fd_core(guest_fd, &host_fd);
  if (filesystem == 1)
    return host_fd;
  if (filesystem < 0)
    return -1;
  const int shared = darwin_art_android_shared_memory_dup(guest_fd);
  return shared == -2 ? Fail(9, -1) : shared;
}

extern "C" int darwin_art_bionic_fd_dup_host_fd_core(int guest_fd,
                                                       int *host_fd) {
  if (host_fd == nullptr)
    return Fail(14, -1);

  bool binder_mapping = false;
  if ((static_cast<uint32_t>(guest_fd) & kCentralBrokerTokenTopMask) ==
      kCentralBrokerTokenMarker) {
    ProcessLease lease;
    Process *process = lease.get();
    if (process == nullptr)
      return Fail(9, -1);
    DarwinArtFdKind kind = DARWIN_ART_FD_FS_FILE;
    const DarwinArtFdBrokerStatus status =
        darwin_art_fd_broker_get_kind(process->broker, guest_fd, &kind);
    if (status != DARWIN_ART_FD_BROKER_OK)
      return Fail(BrokerFailure(status), -1);
    binder_mapping = kind == DARWIN_ART_FD_BINDER;
  }

  const int exported = darwin_art_bionic_fd_export_for_scm(guest_fd);
  if (exported < 0)
    return -1;
  *host_fd = exported;
  return binder_mapping ? 2 : 1;
}

extern "C" int darwin_art_bionic_fd_import_from_scm(int host_fd) {
  PreserveErrno preserve;
  if (host_fd < 0)
    return Fail(9, -1);
  int socket_type = 0;
  socklen_t socket_type_length = sizeof(socket_type);
  const bool is_socket = getsockopt(host_fd, SOL_SOCKET, SO_TYPE, &socket_type,
                                    &socket_type_length) == 0;
  struct stat status{};
  const bool is_pipe =
      !is_socket && fstat(host_fd, &status) == 0 && S_ISFIFO(status.st_mode);
  if (!is_socket && !is_pipe) {
    size_t shared_size = 0;
    int shared_protection = 0;
    if (darwin_art_android_shared_memory_get_info(host_fd, &shared_size,
                                                  &shared_protection) == 1) {
      if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
        std::fprintf(stderr,
                     "ART Binder SCM: import host_fd=%d as=shared-memory "
                     "size=%zu protection=%d\n",
                     host_fd, shared_size, shared_protection);
      }
      return host_fd;
    }
    const int guest_fd = darwin_art_bionic_fs_adopt_host_fd_core(host_fd);
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
      std::fprintf(stderr,
                   "ART Binder SCM: import host_fd=%d mode=0%o as=file "
                   "guest_fd=%d errno=%d\n",
                   host_fd, static_cast<unsigned>(status.st_mode), guest_fd,
                   errno);
    }
    return guest_fd;
  }

  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr) {
    (void)close(host_fd);
    return Fail(9, -1);
  }
  const int flags = fcntl(host_fd, F_GETFL);
  const int fd_flags = fcntl(host_fd, F_GETFD);
  if (flags < 0 || fd_flags < 0 ||
      fcntl(host_fd, F_SETFD, fd_flags | FD_CLOEXEC) != 0) {
    const int error = AndroidErrno(errno);
    (void)close(host_fd);
    return Fail(error, -1);
  }
  auto *object = new (std::nothrow) HostFdObject{host_fd};
  if (object == nullptr) {
    (void)close(host_fd);
    return Fail(12, -1);
  }
  process->objects.fetch_add(1, std::memory_order_release);
  int guest_fd = -1;
  const int status_flags =
      (flags & O_NONBLOCK) != 0 ? DARWIN_ART_FD_STATUS_NONBLOCK : 0;
  const auto broker_status = darwin_art_fd_broker_publish_with_flags(
      process->broker, is_socket ? process->socket_owner : process->pipe_owner,
      reinterpret_cast<uint64_t>(object), status_flags, DARWIN_ART_FD_CLOEXEC,
      &guest_fd);
  if (broker_status != DARWIN_ART_FD_BROKER_OK) {
    int ignored = 0;
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(object), &ignored);
    return Fail(BrokerFailure(broker_status), -1);
  }
  if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
    std::fprintf(stderr,
                 "ART Binder SCM: import host_fd=%d as=%s guest_fd=%d\n",
                 host_fd, is_socket ? "socket" : "pipe", guest_fd);
  }
  return guest_fd;
}

struct ScmExportContext {
  Process* process = nullptr;
  std::vector<std::unique_ptr<darwin_art::bionic::scm::RetainedScmExport>>*
      retained = nullptr;
};

using CarrierExport = darwin_art::bionic::scm::ScopedCarrierExport;

bool CreateCarrierExport(Process *process, int guest_descriptor,
                         std::unique_ptr<CarrierExport> *carrier) {
  if (process == nullptr || carrier == nullptr)
    return false;
  const bool central_namespace =
      (static_cast<uint32_t>(guest_descriptor) & kCentralBrokerTokenTopMask) ==
      kCentralBrokerTokenMarker;
  DarwinArtFdIoResult result{};
  const auto status = CarrierExport::Create(
      process->broker, guest_descriptor, central_namespace,
      &darwin_art_bionic_fd_export_for_scm, carrier, &result);
  if (status == DARWIN_ART_FD_BROKER_OK && *carrier != nullptr)
    return true;
  if (result.android_errno != 0)
    darwin_art_bionic_errno_store(result.android_errno);
  else if (central_namespace)
    darwin_art_bionic_errno_store(BrokerFailure(status));
  return false;
}

int ExportDescriptorForSend(void* opaque, int guest_descriptor,
                            bool* borrowed) {
  auto* context = static_cast<ScmExportContext*>(opaque);
  if (context == nullptr || context->process == nullptr ||
      context->retained == nullptr || borrowed == nullptr)
    return Fail(14, -1);
  *borrowed = false;
  if ((static_cast<uint32_t>(guest_descriptor) & kCentralBrokerTokenTopMask) !=
      kCentralBrokerTokenMarker)
    return darwin_art_bionic_fd_export_for_scm(guest_descriptor);

  std::unique_ptr<darwin_art::bionic::scm::RetainedScmExport> retained;
  DarwinArtFdIoResult result{};
  const auto status =
      darwin_art::bionic::scm::RetainedScmExport::Create(
          context->process->broker, guest_descriptor, &retained, &result);
  if (status != DARWIN_ART_FD_BROKER_OK || retained == nullptr) {
    darwin_art_bionic_errno_store(
        result.android_errno != 0 ? result.android_errno
                                  : BrokerFailure(status));
    return -1;
  }
  const int host_fd = retained->host_fd();
  try {
    context->retained->push_back(std::move(retained));
  } catch (const std::bad_alloc&) {
    darwin_art_bionic_errno_store(12);
    return -1;
  }
  *borrowed = true;
  return host_fd;
}

static intptr_t SendMessageOnHostSocket(
    Process* process, const CarrierExport &carrier, int guest_fd,
    const AndroidMsghdr *android_message, int android_flags) {
  if (android_message == nullptr ||
      (android_message->vector_count != 0 &&
       android_message->vectors == nullptr) ||
      android_message->vector_count > static_cast<size_t>(INT_MAX)) {
    return Fail(android_message == nullptr ? 14 : 95, intptr_t{-1});
  }
  int host_flags = 0;
  if (!TranslateFlags(android_flags, &host_flags))
    return Fail(95, intptr_t{-1});
  sockaddr_storage storage{};
  socklen_t address_length = 0;
  sockaddr* address = nullptr;
  if (android_message->name != nullptr || android_message->name_length != 0) {
    if (!ToHostAddress(android_message->name, android_message->name_length,
                       &storage, &address_length)) {
      return Fail(22, intptr_t{-1});
    }
    address = reinterpret_cast<sockaddr*>(&storage);
  }
  std::vector<iovec> vectors(android_message->vector_count);
  for (size_t index = 0; index < vectors.size(); ++index) {
    vectors[index].iov_base = android_message->vectors[index].base;
    vectors[index].iov_len = android_message->vectors[index].length;
  }
  const int host_socket = carrier.host_fd();
  darwin_art::bionic::scm::ExportedRights owned_exports;
  std::vector<std::unique_ptr<darwin_art::bionic::scm::RetainedScmExport>>
      retained_exports;
  ScmExportContext export_context{process, &retained_exports};
  const auto export_status = owned_exports.DecodeRetained(
      android_message->control, android_message->control_length,
      &ExportDescriptorForSend, &export_context);
  using ExportStatus = darwin_art::bionic::scm::ExportStatus;
  switch (export_status) {
    case ExportStatus::Ok: break;
    case ExportStatus::DescriptorFailure: return -1;
    case ExportStatus::MissingControl: return Fail(14, intptr_t{-1});
    case ExportStatus::InvalidControl: return Fail(22, intptr_t{-1});
    case ExportStatus::Unsupported: return Fail(95, intptr_t{-1});
    case ExportStatus::OutOfMemory: return Fail(12, intptr_t{-1});
  }
  const auto exported = owned_exports.descriptors();
  std::vector<uint8_t> control;
  if (!exported.empty()) {
    control.resize(CMSG_SPACE(exported.size() * sizeof(int)));
  }
  msghdr message{};
  message.msg_name = address;
  message.msg_namelen = address_length;
  message.msg_iov = vectors.data();
  message.msg_iovlen = static_cast<int>(vectors.size());
  if (!control.empty()) {
    message.msg_control = control.data();
    message.msg_controllen = control.size();
    cmsghdr *header = CMSG_FIRSTHDR(&message);
    header->cmsg_level = SOL_SOCKET;
    header->cmsg_type = SCM_RIGHTS;
    header->cmsg_len = CMSG_LEN(exported.size() * sizeof(int));
    std::memcpy(CMSG_DATA(header), exported.data(),
                exported.size() * sizeof(int));
  }
  ssize_t result;
  do {
    result = ::sendmsg(host_socket, &message, host_flags);
  } while (result < 0 && errno == EINTR);
  const int error = errno;
  if (SocketDebugEnabled()) {
    size_t payload_bytes = 0;
    for (const iovec &vector : vectors)
      payload_bytes += vector.iov_len;
    std::fprintf(stderr,
                 "DARWIN socket: pid=%d tid=%llu sendmsg fd=%d host_fd=%d vectors=%zu "
                 "payload=%zu rights=%zu flags=%#x result=%zd errno=%d\n",
                 getpid(), static_cast<unsigned long long>(SocketDebugThreadId()),
                 guest_fd, host_socket, vectors.size(), payload_bytes,
                 exported.size(), android_flags, result,
                 result < 0 ? error : 0);
  }
  owned_exports.Clear();
  if (result < 0)
    return Fail(AndroidErrno(error), intptr_t{-1});
  return static_cast<intptr_t>(result);
}

extern "C" intptr_t darwin_art_bionic_socket_broker_sendmsg(
    int fd, const AndroidMsghdr *android_message, int android_flags) {
  PreserveErrno preserve;
  ProcessLease process_lease;
  Process* process = process_lease.get();
  if (process == nullptr)
    return Fail(38, intptr_t{-1});
  std::unique_ptr<CarrierExport> carrier;
  if (!CreateCarrierExport(process, fd, &carrier))
    return -1;
  const intptr_t result = SendMessageOnHostSocket(
      process, *carrier, fd, android_message, android_flags);
  return result;
}

extern "C" int darwin_art_bionic_socket_broker_sendmmsg(
    int fd, AndroidMmsghdr* messages, uint32_t count, int flags) {
  PreserveErrno preserve;
  ProcessLease process_lease;
  Process* process = process_lease.get();
  if (process == nullptr)
    return Fail(38, -1);
  // Linux validates the descriptor even for an empty batch, then treats a
  // larger vlen as a bounded batch rather than returning EINVAL.
  if (count == 0) {
    std::unique_ptr<CarrierExport> carrier;
    if (!CreateCarrierExport(process, fd, &carrier))
      return -1;
    int socket_type = 0;
    socklen_t socket_type_length = sizeof(socket_type);
    const int socket_result = getsockopt(
        carrier->host_fd(), SOL_SOCKET, SO_TYPE, &socket_type,
        &socket_type_length);
    const int socket_errno = errno;
    if (socket_result < 0) return Fail(AndroidErrno(socket_errno), -1);
    return 0;
  }
  if (messages == nullptr) return Fail(14, -1);
  const uint32_t limit = std::min(count, kAndroidSendMmsgMax);
  std::unique_ptr<CarrierExport> carrier;
  if (!CreateCarrierExport(process, fd, &carrier))
    return -1;

  const int32_t prior_errno = darwin_art_bionic_errno_load();
  uint32_t sent = 0;
  while (sent < limit) {
    uint64_t requested = 0;
    bool requested_known = true;
    const AndroidMsghdr& message = messages[sent].msg_hdr;
    if (message.vector_count != 0 && message.vectors == nullptr) {
      requested_known = false;
    } else if (message.vector_count > static_cast<uint64_t>(INT_MAX)) {
      requested_known = false;
    } else {
      for (uint64_t index = 0; index < message.vector_count; ++index) {
        const uint64_t length = message.vectors[index].length;
        if (UINT64_MAX - requested < length) {
          requested_known = false;
          break;
        }
        requested += length;
      }
    }
    const intptr_t result = SendMessageOnHostSocket(
        process, *carrier, fd, &messages[sent].msg_hdr, flags);
    if (result < 0) {
      if (sent != 0) {
        // sendmmsg returns the number of completed messages after a partial
        // batch; errno is not part of that successful return.
        darwin_art_bionic_errno_store(prior_errno);
        return static_cast<int>(sent);
      }
      return -1;
    }
    if (static_cast<uint64_t>(result) > UINT32_MAX) {
      if (sent != 0) {
        darwin_art_bionic_errno_store(prior_errno);
        return static_cast<int>(sent);
      }
      return Fail(75, -1);
    }
    messages[sent].msg_len = static_cast<uint32_t>(result);
    ++sent;
    // A stream socket may send only part of one message. Linux stops the
    // batch at that point and does not attempt later messages.
    if (requested_known && static_cast<uint64_t>(result) < requested) break;
  }
  return static_cast<int>(sent);
}

extern "C" intptr_t
darwin_art_bionic_socket_broker_recvmsg(int fd, AndroidMsghdr *android_message,
                                        int android_flags) {
  PreserveErrno preserve;
  if (android_message == nullptr ||
      (android_message->vector_count != 0 &&
       android_message->vectors == nullptr) ||
      android_message->vector_count > static_cast<size_t>(INT_MAX) ||
      android_message->name != nullptr ||
      (android_message->control_length != 0 &&
       android_message->control == nullptr)) {
    return Fail(android_message == nullptr ? 14 : 95, intptr_t{-1});
  }
  int host_flags = 0;
  if (!TranslateFlags(android_flags, &host_flags))
    return Fail(95, intptr_t{-1});
  int32_t pass_credentials = 0;
  uint32_t pass_credentials_length = sizeof(pass_credentials);
  if (darwin_art_bionic_socket_broker_getsockopt(
          fd, kAndroidSolSocket, kAndroidSoPasscred, &pass_credentials,
          &pass_credentials_length) != 0) {
    pass_credentials = 0;
  }
  std::vector<iovec> vectors(android_message->vector_count);
  for (size_t index = 0; index < vectors.size(); ++index) {
    vectors[index].iov_base = android_message->vectors[index].base;
    vectors[index].iov_len = android_message->vectors[index].length;
  }
  std::vector<uint8_t> host_control(android_message->control_length);
  msghdr message{};
  message.msg_iov = vectors.data();
  message.msg_iovlen = static_cast<int>(vectors.size());
  message.msg_control = host_control.empty() ? nullptr : host_control.data();
  message.msg_controllen = host_control.size();
  const int host_socket = darwin_art_bionic_fd_export_for_scm(fd);
  if (host_socket < 0)
    return -1;
  ssize_t result;
  do {
    result = ::recvmsg(host_socket, &message, host_flags);
  } while (result < 0 && errno == EINTR);
  const int error = errno;
  if (SocketDebugEnabled()) {
    size_t rights = 0;
    for (cmsghdr *header = CMSG_FIRSTHDR(&message); header != nullptr;
         header = CMSG_NXTHDR(&message, header)) {
      if (header->cmsg_level == SOL_SOCKET && header->cmsg_type == SCM_RIGHTS &&
          header->cmsg_len >= CMSG_LEN(0)) {
        rights += (header->cmsg_len - CMSG_LEN(0)) / sizeof(int);
      }
    }
    std::fprintf(stderr,
                 "DARWIN socket: pid=%d tid=%llu recvmsg fd=%d host_fd=%d vectors=%zu "
                 "control_capacity=%zu rights=%zu flags=%#x result=%zd "
                 "msg_flags=%#x errno=%d\n",
                 getpid(), static_cast<unsigned long long>(SocketDebugThreadId()),
                 fd, host_socket, vectors.size(), host_control.size(), rights,
                 android_flags, result, message.msg_flags,
                 result < 0 ? error : 0);
  }
  (void)close(host_socket);
  if (result < 0)
    return Fail(AndroidErrno(error), intptr_t{-1});

  android_message->flags = message.msg_flags;
  android_message->name_length = 0;
  size_t output_offset = 0;
  for (cmsghdr *header = CMSG_FIRSTHDR(&message); header != nullptr;
       header = CMSG_NXTHDR(&message, header)) {
    if (header->cmsg_level != SOL_SOCKET || header->cmsg_type != SCM_RIGHTS)
      continue;
    const size_t payload = header->cmsg_len - CMSG_LEN(0);
    const size_t count = payload / sizeof(int);
    const size_t android_length = sizeof(AndroidCmsghdr) + payload;
    constexpr size_t alignment = sizeof(size_t);
    const size_t android_space =
        (android_length + alignment - 1) & ~(alignment - 1);
    auto *received_fds = reinterpret_cast<int *>(CMSG_DATA(header));
    if (output_offset + android_space > android_message->control_length) {
      for (size_t index = 0; index < count; ++index)
        (void)close(received_fds[index]);
      android_message->flags |= MSG_CTRUNC;
      continue;
    }
    auto *android_header = reinterpret_cast<AndroidCmsghdr *>(
        static_cast<uint8_t *>(android_message->control) + output_offset);
    android_header->length = android_length;
    android_header->level = kAndroidSolSocket;
    android_header->type = SCM_RIGHTS;
    auto *android_fds = reinterpret_cast<int *>(
        reinterpret_cast<uint8_t *>(android_header) + sizeof(AndroidCmsghdr));
    for (size_t index = 0; index < count; ++index) {
      android_fds[index] =
          darwin_art_bionic_fd_import_from_scm(received_fds[index]);
      if (android_fds[index] < 0) {
        for (size_t remaining = index + 1; remaining < count; ++remaining)
          (void)close(received_fds[remaining]);
        android_message->control_length = output_offset;
        return -1;
      }
    }
    output_offset += android_space;
  }
  if (pass_credentials != 0) {
    constexpr size_t alignment = sizeof(size_t);
    constexpr size_t credential_length =
        sizeof(AndroidCmsghdr) + sizeof(AndroidUcred);
    constexpr size_t credential_space =
        (credential_length + alignment - 1) & ~(alignment - 1);
    if (output_offset + credential_space <= android_message->control_length) {
      auto *android_header = reinterpret_cast<AndroidCmsghdr *>(
          static_cast<uint8_t *>(android_message->control) + output_offset);
      android_header->length = credential_length;
      android_header->level = kAndroidSolSocket;
      android_header->type = 2; // Linux SCM_CREDENTIALS.
      const AndroidUcred credentials{static_cast<int32_t>(getpid()), getuid(),
                                     getgid()};
      std::memcpy(reinterpret_cast<uint8_t *>(android_header) +
                      sizeof(AndroidCmsghdr),
                  &credentials, sizeof(credentials));
      output_offset += credential_space;
      if (SocketDebugEnabled()) {
        std::fprintf(stderr,
                     "DARWIN socket: recvmsg fd=%d synthesized "
                     "SCM_CREDENTIALS pid=%d uid=%u gid=%u\n",
                     fd, credentials.process_id, credentials.user_id,
                     credentials.group_id);
      }
    } else {
      android_message->flags |= MSG_CTRUNC;
    }
  }
  android_message->control_length = output_offset;
  return static_cast<intptr_t>(result);
}

extern "C" int darwin_art_bionic_socket_broker_getaddrinfo(
    const char *node, const char *service,
    const DarwinArtAndroidAddrinfo *hints, DarwinArtAndroidAddrinfo **result) {
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr) {
    if (result != nullptr)
      *result = nullptr;
    return 11;
  }
  const int status =
      darwin_art_bionic_dns_getaddrinfo(node, service, hints, result);
  if (status == 0 && result != nullptr && *result != nullptr)
    process->dns_results.fetch_add(1, std::memory_order_acq_rel);
  return status;
}

extern "C" void
darwin_art_bionic_socket_broker_freeaddrinfo(DarwinArtAndroidAddrinfo *result) {
  PreserveErrno preserve;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return;
  darwin_art_bionic_dns_freeaddrinfo(result);
  if (result != nullptr) {
    const size_t previous =
        process->dns_results.fetch_sub(1, std::memory_order_acq_rel);
    if (previous == 0)
      std::abort();
  }
}

extern "C" int darwin_art_bionic_socket_broker_res_nquery(uint64_t network,
                                                          const char *name,
                                                          int dns_class,
                                                          int dns_type,
                                                          uint32_t flags) {
  PreserveErrno preserve;
  (void)network;
  constexpr uint32_t kKnownFlags = 0x7;
  if (name == nullptr || dns_class <= 0 || dns_type <= 0 ||
      (flags & ~kKnownFlags) != 0)
    return -22;
  if (std::getenv("DARWIN_ART_DEBUG_DNS") != nullptr) {
    std::fprintf(stderr, "DARWIN DNS query name=%s class=%d type=%d flags=%u\n",
                 name, dns_class, dns_type, flags);
  }
  const size_t name_length = std::strlen(name);
  if (name_length > 255)
    return -90;
  size_t label_length = 0;
  for (size_t index = 0; index <= name_length; ++index) {
    if (index == name_length || name[index] == '.') {
      if (label_length > 63)
        return -90;
      label_length = 0;
    } else {
      ++label_length;
    }
  }

  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr || !RetainProcess(process))
    return -38;

  int host_descriptors[2] = {-1, -1};
  if (native_fd::CreateCloseOnExecPipe(host_descriptors) != 0 ||
      fcntl(host_descriptors[0], F_SETFD, FD_CLOEXEC) != 0 ||
      fcntl(host_descriptors[1], F_SETFD, FD_CLOEXEC) != 0) {
    const int error = host_descriptors[0] < 0 ? AndroidErrno(errno) : 5;
    if (host_descriptors[0] >= 0)
      (void)close(host_descriptors[0]);
    if (host_descriptors[1] >= 0)
      (void)close(host_descriptors[1]);
    ReleaseProcess(process);
    return -error;
  }
  auto state = std::make_shared<DnsQueryState>();
  auto *object = new (std::nothrow)
      HostFdObject{host_descriptors[0], host_descriptors[1], nullptr, state};
  if (object == nullptr) {
    (void)close(host_descriptors[0]);
    (void)close(host_descriptors[1]);
    ReleaseProcess(process);
    return -12;
  }
  process->objects.fetch_add(1, std::memory_order_release);
  int guest_fd = -1;
  const DarwinArtFdBrokerStatus publish_status =
      darwin_art_fd_broker_publish_with_flags(
          process->broker, process->pipe_owner,
          reinterpret_cast<uint64_t>(object), 0, DARWIN_ART_FD_CLOEXEC,
          &guest_fd);
  if (publish_status != DARWIN_ART_FD_BROKER_OK) {
    int ignored = 0;
    (void)OwnerClose(process, reinterpret_cast<uint64_t>(object), &ignored);
    ReleaseProcess(process);
    return -BrokerFailure(publish_status);
  }
  state->guest_fd = guest_fd;
  state->signal_fd = host_descriptors[1];
  {
    std::lock_guard lock(process->dns_mutex);
    process->dns_queries.emplace(guest_fd, state);
  }
  process->async_dns_queries.fetch_add(1, std::memory_order_release);

  try {
    std::thread([process, state, query_name = std::string(name), dns_class,
                 dns_type, flags] {
      std::vector<uint8_t> query(NS_PACKETSZ);
      std::vector<uint8_t> answer(NS_MAXMSG);
      struct __res_state resolver{};
      int error = 0;
      int answer_length = -1;
      if (res_ninit(&resolver) != 0) {
        error = -5;
      } else {
        if ((flags & 0x1) != 0)
          resolver.retry = 1;
        const int query_length = res_nmkquery(
            &resolver, ns_o_query, query_name.c_str(), dns_class, dns_type,
            nullptr, 0, nullptr, query.data(), static_cast<int>(query.size()));
        if (query_length < 0) {
          error = -90;
        } else {
          answer_length =
              res_nsend(&resolver, query.data(), query_length, answer.data(),
                        static_cast<int>(answer.size()));
          if (answer_length < 0)
            error = -5;
        }
        res_nclose(&resolver);
      }
      {
        std::lock_guard lock(state->mutex);
        if (!state->cancelled) {
          state->error = error;
          if (answer_length >= 0) {
            answer.resize(static_cast<size_t>(answer_length));
            state->answer = std::move(answer);
            state->rcode =
                state->answer.size() >= 4 ? state->answer[3] & 0x0f : 0;
          }
          state->complete = true;
          if (state->signal_fd >= 0) {
            const uint8_t ready = 1;
            (void)write(state->signal_fd, &ready, sizeof(ready));
          }
          if (std::getenv("DARWIN_ART_DEBUG_DNS") != nullptr) {
            std::fprintf(
                stderr,
                "DARWIN DNS complete fd=%d bytes=%zu rcode=%d error=%d\n",
                state->guest_fd, state->answer.size(), state->rcode,
                state->error);
          }
        }
      }
      ReleaseProcess(process);
    }).detach();
  } catch (...) {
    (void)darwin_art_bionic_socket_broker_close(guest_fd);
    ReleaseProcess(process);
    return -11;
  }
  return guest_fd;
}

extern "C" int
darwin_art_bionic_socket_broker_res_nresult(int fd, int *rcode, uint8_t *answer,
                                            size_t answer_capacity) {
  PreserveErrno preserve;
  if (rcode == nullptr || (answer == nullptr && answer_capacity != 0))
    return -14;
  ProcessLease lease;
  Process *process = lease.get();
  if (process == nullptr)
    return -9;
  std::shared_ptr<DnsQueryState> state;
  {
    std::lock_guard lock(process->dns_mutex);
    const auto found = process->dns_queries.find(fd);
    if (found == process->dns_queries.end())
      return -9;
    state = found->second;
  }
  int result = 0;
  {
    std::lock_guard lock(state->mutex);
    if (!state->complete) {
      result = -11;
    } else if (state->error != 0) {
      result = state->error;
    } else if (state->answer.size() > answer_capacity) {
      result = -90;
    } else {
      if (!state->answer.empty())
        std::memcpy(answer, state->answer.data(), state->answer.size());
      *rcode = state->rcode;
      result = static_cast<int>(state->answer.size());
    }
  }
  if (darwin_art_bionic_socket_broker_close(fd) != 0 && result >= 0)
    return -9;
  if (std::getenv("DARWIN_ART_DEBUG_DNS") != nullptr) {
    std::fprintf(stderr, "DARWIN DNS result fd=%d result=%d\n", fd, result);
  }
  return result;
}

extern "C" intptr_t darwin_art_bionic_socket_broker___sendto_chk(
    int fd, const void *bytes, size_t count, int flags, const void *address,
    uint32_t address_length, size_t buffer_size) {
  if (count > buffer_size)
    return Fail(34, static_cast<intptr_t>(-1));
  return darwin_art_bionic_socket_broker_sendto(fd, bytes, count, flags,
                                                address, address_length);
}

extern "C" int darwin_art_bionic_socket_broker_getifaddrs(void **result) {
  if (result != nullptr)
    *result = nullptr;
  return Fail(38, -1);
}

extern "C" void darwin_art_bionic_socket_broker_freeifaddrs(void *result) {
  (void)result;
}

extern "C" char *darwin_art_bionic_socket_broker_if_indextoname(unsigned index,
                                                                char *name) {
  if (index != 1 || name == nullptr) {
    Fail(6, -1);
    return nullptr;
  }
  name[0] = 'l';
  name[1] = 'o';
  name[2] = '\0';
  return name;
}

extern "C" DarwinArtBionicSocketBrokerFunction
darwin_art_bionic_socket_broker_resolve(const char *soname, const char *symbol,
                                        const char *version) {
  if (const auto fdsan = darwin_art_bionic_socket_broker_fdsan_resolve(
          soname, symbol, version)) {
    return fdsan;
  }
  const bool interface_version_alias =
      symbol != nullptr && version != nullptr &&
      (std::strcmp(symbol, "freeifaddrs") == 0 ||
       std::strcmp(symbol, "getifaddrs") == 0) &&
      std::strcmp(version, "LIBC_N") == 0;
  const bool socket_version_alias =
      symbol != nullptr && version != nullptr &&
      (((std::strcmp(symbol, "__write_chk") == 0 &&
         std::strcmp(version, "LIBC_N") == 0)) ||
       (std::strcmp(symbol, "__sendto_chk") == 0 &&
        std::strcmp(version, "LIBC_O") == 0) ||
       ((std::strcmp(symbol, "android_fdsan_close_with_tag") == 0 ||
         std::strcmp(symbol, "android_fdsan_create_owner_tag") == 0 ||
         std::strcmp(symbol, "android_fdsan_exchange_owner_tag") == 0) &&
        std::strcmp(version, "LIBC_Q") == 0));
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::strcmp(soname, "libc.so") != 0 ||
      (std::strcmp(version, "LIBC") != 0 && !interface_version_alias &&
       !socket_version_alias))
    return nullptr;
  if (std::strcmp(symbol, "socket") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_socket);
  if (std::strcmp(symbol, "__sendto_chk") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker___sendto_chk);
  if (std::strcmp(symbol, "__read_chk") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker___read_chk);
  if (std::strcmp(symbol, "__write_chk") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker___write_chk);
  if (std::strcmp(symbol, "android_fdsan_close_with_tag") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_android_fdsan_close_with_tag);
  if (std::strcmp(symbol, "android_fdsan_create_owner_tag") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_android_fdsan_create_owner_tag);
  if (std::strcmp(symbol, "android_fdsan_exchange_owner_tag") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_android_fdsan_exchange_owner_tag);
  if (std::strcmp(symbol, "freeifaddrs") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_freeifaddrs);
  if (std::strcmp(symbol, "getifaddrs") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_getifaddrs);
  if (std::strcmp(symbol, "if_indextoname") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_if_indextoname);
  if (std::strcmp(symbol, "__cmsg_nxthdr") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_cmsg_nxthdr);
  if (std::strcmp(symbol, "if_nametoindex") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_if_nametoindex);
  if (std::strcmp(symbol, "socketpair") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_socketpair);
  if (std::strcmp(symbol, "pipe") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_pipe);
  if (std::strcmp(symbol, "pipe2") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_pipe2);
  if (std::strcmp(symbol, "eventfd") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_eventfd);
  if (std::strcmp(symbol, "timerfd_create") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_timerfd_create);
  if (std::strcmp(symbol, "timerfd_settime") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_timerfd_settime);
  if (std::strcmp(symbol, "epoll_create") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_epoll_create);
  if (std::strcmp(symbol, "epoll_create1") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_epoll_create1);
  if (std::strcmp(symbol, "epoll_ctl") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_epoll_ctl);
  if (std::strcmp(symbol, "epoll_wait") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_epoll_wait);
  if (std::strcmp(symbol, "dup") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_dup);
  if (std::strcmp(symbol, "dup2") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_dup2);
  if (std::strcmp(symbol, "select") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_select);
  if (std::strcmp(symbol, "readv") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_readv);
  if (std::strcmp(symbol, "writev") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_writev);
  if (std::strcmp(symbol, "read") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_read);
  if (std::strcmp(symbol, "write") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_write);
  if (std::strcmp(symbol, "poll") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_poll);
  if (std::strcmp(symbol, "connect") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_connect);
  if (std::strcmp(symbol, "bind") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_bind);
  if (std::strcmp(symbol, "listen") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_listen);
  if (std::strcmp(symbol, "accept4") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_accept4);
  if (std::strcmp(symbol, "accept") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_accept);
  if (std::strcmp(symbol, "getsockname") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_getsockname);
  if (std::strcmp(symbol, "getpeername") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_getpeername);
  if (std::strcmp(symbol, "send") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_send);
  if (std::strcmp(symbol, "recv") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_recv);
  if (std::strcmp(symbol, "sendto") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_sendto);
  if (std::strcmp(symbol, "recvfrom") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_recvfrom);
  if (std::strcmp(symbol, "sendmsg") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_sendmsg);
  if (std::strcmp(symbol, "sendmmsg") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_sendmmsg);
  if (std::strcmp(symbol, "recvmsg") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_recvmsg);
  if (std::strcmp(symbol, "recvmmsg") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_message_unsupported);
  if (std::strcmp(symbol, "getsockopt") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_getsockopt);
  if (std::strcmp(symbol, "setsockopt") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_setsockopt);
  if (std::strcmp(symbol, "shutdown") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_shutdown);
  if (std::strcmp(symbol, "close") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_close);
  if (std::strcmp(symbol, "fcntl") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_fcntl);
  return nullptr;
}

extern "C" uintptr_t darwin_art_bionic_socket_broker_data_resolve(
    const char *soname, const char *symbol, const char *version) {
  static constexpr uint8_t kIn6AddrAny[16] = {};
  static constexpr uint8_t kIn6AddrLoopback[16] = {0, 0, 0, 0, 0, 0, 0, 0,
                                                   0, 0, 0, 0, 0, 0, 0, 1};
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::strcmp(soname, "libc.so") != 0 ||
      std::strcmp(version, "LIBC_N") != 0)
    return 0;
  if (std::strcmp(symbol, "in6addr_any") == 0)
    return reinterpret_cast<uintptr_t>(kIn6AddrAny);
  if (std::strcmp(symbol, "in6addr_loopback") == 0)
    return reinterpret_cast<uintptr_t>(kIn6AddrLoopback);
  return 0;
}

extern "C" DarwinArtBionicSocketBrokerFunction
darwin_art_bionic_socket_broker_dns_resolve(const char *soname,
                                            const char *symbol,
                                            const char *version) {
  if (soname != nullptr && symbol != nullptr && version == nullptr &&
      std::strcmp(soname, "libandroid.so") == 0) {
    if (std::strcmp(symbol, "android_res_nquery") == 0)
      return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
          &darwin_art_bionic_socket_broker_res_nquery);
    if (std::strcmp(symbol, "android_res_nresult") == 0)
      return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
          &darwin_art_bionic_socket_broker_res_nresult);
  }
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::strcmp(soname, "libc.so") != 0 || std::strcmp(version, "LIBC") != 0)
    return nullptr;
  if (std::strcmp(symbol, "getaddrinfo") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_getaddrinfo);
  if (std::strcmp(symbol, "android_getaddrinfofornet") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_dns_android_getaddrinfofornet);
  if (std::strcmp(symbol, "gethostbyname") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_gethostbyname);
  if (std::strcmp(symbol, "__get_h_errno") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_dns_get_h_errno);
  if (std::strcmp(symbol, "getservbyname") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_dns_getservbyname);
  if (std::strcmp(symbol, "gethostbyaddr") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_gethostbyaddr);
  if (std::strcmp(symbol, "freeaddrinfo") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_socket_broker_freeaddrinfo);
  if (std::strcmp(symbol, "gai_strerror") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_dns_gai_strerror);
  if (std::strcmp(symbol, "inet_ntop") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_dns_inet_ntop);
  if (std::strcmp(symbol, "getnameinfo") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_dns_getnameinfo);
  if (std::strcmp(symbol, "inet_pton") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_dns_inet_pton);
  if (std::strcmp(symbol, "inet_addr") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_dns_inet_addr);
  if (std::strcmp(symbol, "inet_ntoa") == 0)
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_dns_inet_ntoa);
  return nullptr;
}

extern "C" size_t darwin_art_bionic_socket_broker_live_objects() {
  std::lock_guard global(g_process_mutex);
  return g_process == nullptr
             ? 0
             : g_process->objects.load(std::memory_order_acquire);
}

extern "C" int darwin_art_bionic_socket_broker_is_active() {
  std::lock_guard global(g_process_mutex);
  return g_process == nullptr ? 0 : 1;
}
