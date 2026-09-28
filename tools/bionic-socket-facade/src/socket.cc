#include "aim_bionic_socket.h"

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <pthread.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>

extern "C" void aim_bionic_errno_store(int32_t android_errno);

namespace {

constexpr size_t kMaxSockets = 512;
constexpr uint32_t kTokenMarker = 0x40000000U;
constexpr uint32_t kTokenIndexBits = 9;
constexpr uint32_t kTokenIndexMask = (1U << kTokenIndexBits) - 1U;
constexpr uint32_t kTokenGenerationMask = 0x1fffffU;
constexpr int kAndroidAfUnspec = 0;
constexpr int kAndroidAfUnix = 1;
constexpr int kAndroidAfInet = 2;
constexpr int kAndroidAfInet6 = 10;
constexpr int kAndroidSockStream = 1;
constexpr int kAndroidSockDgram = 2;
constexpr int kAndroidSockNonblock = 00004000;
constexpr int kAndroidSockCloexec = 02000000;
constexpr int kAndroidSolSocket = 1;
constexpr int kAndroidSoReuseaddr = 2;
constexpr int kAndroidSoError = 4;
constexpr int kAndroidSoSndbuf = 7;
constexpr int kAndroidSoRcvbuf = 8;
constexpr int kAndroidSoKeepalive = 9;
constexpr int kAndroidSoType = 3;
constexpr int kAndroidMsgOob = 0x1;
constexpr int kAndroidMsgPeek = 0x2;
constexpr int kAndroidMsgDontRoute = 0x4;
constexpr int kAndroidMsgCtrunc = 0x8;
constexpr int kAndroidMsgTrunc = 0x20;
constexpr int kAndroidMsgDontWait = 0x40;
constexpr int kAndroidMsgEor = 0x80;
constexpr int kAndroidMsgWaitAll = 0x100;
constexpr int kAndroidMsgNoSignal = 0x4000;
constexpr int16_t kAndroidPollIn = 0x001;
constexpr int16_t kAndroidPollPri = 0x002;
constexpr int16_t kAndroidPollOut = 0x004;
constexpr int16_t kAndroidPollErr = 0x008;
constexpr int16_t kAndroidPollHup = 0x010;
constexpr int16_t kAndroidPollNval = 0x020;
constexpr int16_t kAndroidPollRdNorm = 0x040;
constexpr int16_t kAndroidPollRdBand = 0x080;
constexpr int16_t kAndroidPollWrNorm = 0x100;
constexpr int16_t kAndroidPollWrBand = 0x200;

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

struct Slot {
  int host_fd = -1;
  uint32_t generation = 0;
};

pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
std::array<Slot, kMaxSockets> g_slots{};
uint32_t g_generation = 1;
size_t g_borrowed_descriptors = 0;

class PreserveErrno {
 public:
  PreserveErrno() : saved_(errno) {}
  ~PreserveErrno() { errno = saved_; }

 private:
  int saved_;
};

int32_t AndroidErrno(int error) {
  switch (error) {
    case 0: return 0;
    case EPERM: return 1;
    case ENOENT: return 2;
    case EINTR: return 4;
    case EIO: return 5;
    case EBADF: return 9;
    case EAGAIN: return 11;
    case ENOMEM: return 12;
    case EACCES: return 13;
    case EFAULT: return 14;
    case EBUSY: return 16;
    case EEXIST: return 17;
    case EINVAL: return 22;
    case EMFILE: return 24;
    case ENOTTY: return 25;
    case ENOSPC: return 28;
    case EPIPE: return 32;
    case EDOM: return 33;
    case ERANGE: return 34;
    case EDEADLK: return 35;
    case ENAMETOOLONG: return 36;
    case ENOSYS: return 38;
    case ENOTEMPTY: return 39;
    case ELOOP: return 40;
    case ENOMSG: return 42;
    case EOVERFLOW: return 75;
    case EILSEQ: return 84;
    case ENOTSOCK: return 88;
    case EDESTADDRREQ: return 89;
    case EMSGSIZE: return 90;
    case EPROTOTYPE: return 91;
    case ENOPROTOOPT: return 92;
    case EPROTONOSUPPORT: return 93;
    case ESOCKTNOSUPPORT: return 94;
    case EOPNOTSUPP: return 95;
    case EAFNOSUPPORT: return 97;
    case EADDRINUSE: return 98;
    case EADDRNOTAVAIL: return 99;
    case ENETDOWN: return 100;
    case ENETUNREACH: return 101;
    case ENETRESET: return 102;
    case ECONNABORTED: return 103;
    case ECONNRESET: return 104;
    case ENOBUFS: return 105;
    case EISCONN: return 106;
    case ENOTCONN: return 107;
    case ETIMEDOUT: return 110;
    case ECONNREFUSED: return 111;
    case EHOSTUNREACH: return 113;
    case EALREADY: return 114;
    case EINPROGRESS: return 115;
    default: return 5;
  }
}

template <typename T>
T Fail(int32_t error, T value) {
  aim_bionic_errno_store(error);
  return value;
}

int FailHost() { return Fail(AndroidErrno(errno), -1); }

int SetDescriptorFlags(int fd, bool nonblocking) {
  if (fcntl(fd, F_SETFD, FD_CLOEXEC) == -1) return -1;
  if (nonblocking) {
    const int old_flags = fcntl(fd, F_GETFL);
    if (old_flags == -1 || fcntl(fd, F_SETFL, old_flags | O_NONBLOCK) == -1) {
      return -1;
    }
  }
#ifdef SO_NOSIGPIPE
  int one = 1;
  if (setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof(one)) == -1) {
    return -1;
  }
#endif
  return 0;
}

int Install(int host_fd) {
  (void)pthread_mutex_lock(&g_lock);
  for (size_t index = 0; index < g_slots.size(); ++index) {
    if (g_slots[index].host_fd != -1) continue;
    uint32_t generation = g_generation++ & kTokenGenerationMask;
    if (generation == 0) generation = g_generation++ & kTokenGenerationMask;
    g_slots[index] = Slot{host_fd, generation};
    const uint32_t encoded = kTokenMarker |
                             (generation << kTokenIndexBits) |
                             static_cast<uint32_t>(index);
    (void)pthread_mutex_unlock(&g_lock);
    return static_cast<int>(encoded);
  }
  (void)pthread_mutex_unlock(&g_lock);
  (void)close(host_fd);
  return Fail(24, -1);
}

bool Decode(int token, size_t* index, uint32_t* generation) {
  const uint32_t encoded = static_cast<uint32_t>(token);
  if (token < 0 || (encoded & kTokenMarker) == 0) {
    return false;
  }
  *index = static_cast<size_t>(encoded & kTokenIndexMask);
  *generation = (encoded >> kTokenIndexBits) & kTokenGenerationMask;
  if (*index >= kMaxSockets || *generation == 0) return false;
  return true;
}

int BorrowRaw(int token, int32_t* android_error) {
  size_t index = 0;
  uint32_t generation = 0;
  if (!Decode(token, &index, &generation)) {
    *android_error = 9;
    return -1;
  }
  (void)pthread_mutex_lock(&g_lock);
  const int original = g_slots[index].host_fd;
  if (original == -1 || g_slots[index].generation != generation) {
    (void)pthread_mutex_unlock(&g_lock);
    *android_error = 9;
    return -1;
  }
  const int duplicate = fcntl(original, F_DUPFD_CLOEXEC, 0);
  const int duplicate_errno = errno;
  if (duplicate != -1) ++g_borrowed_descriptors;
  (void)pthread_mutex_unlock(&g_lock);
  if (duplicate == -1) {
    *android_error = AndroidErrno(duplicate_errno);
    return -1;
  }
  *android_error = 0;
  return duplicate;
}

int Borrow(int token) {
  int32_t android_error = 0;
  const int duplicate = BorrowRaw(token, &android_error);
  if (duplicate == -1) return Fail(android_error, -1);
  return duplicate;
}

void ReleaseBorrow(int descriptor) {
  (void)close(descriptor);
  (void)pthread_mutex_lock(&g_lock);
  if (g_borrowed_descriptors == 0) __builtin_trap();
  --g_borrowed_descriptors;
  (void)pthread_mutex_unlock(&g_lock);
}

bool TranslateDomain(int android, int* host) {
  switch (android) {
    case kAndroidAfUnspec: *host = AF_UNSPEC; return true;
    case kAndroidAfUnix: *host = AF_UNIX; return true;
    case kAndroidAfInet: *host = AF_INET; return true;
    case kAndroidAfInet6: *host = AF_INET6; return true;
    default: return false;
  }
}

bool TranslateType(int android, int* host, bool* nonblocking) {
  constexpr int kKnown = kAndroidSockNonblock | kAndroidSockCloexec;
  if ((android & ~(kKnown | 0xf)) != 0) return false;
  switch (android & 0xf) {
    case kAndroidSockStream: *host = SOCK_STREAM; break;
    case kAndroidSockDgram: *host = SOCK_DGRAM; break;
    default: return false;
  }
  *nonblocking = (android & kAndroidSockNonblock) != 0;
  return true;
}

bool TranslateProtocol(int android, int* host) {
  switch (android) {
    case 0: *host = 0; return true;
    case 6: *host = IPPROTO_TCP; return true;
    case 17: *host = IPPROTO_UDP; return true;
    default: return false;
  }
}

bool TranslateFlags(int android, int* host) {
  constexpr int kKnown = kAndroidMsgOob | kAndroidMsgPeek |
                         kAndroidMsgDontRoute | kAndroidMsgDontWait |
                         kAndroidMsgEor | kAndroidMsgWaitAll |
                         kAndroidMsgNoSignal;
  if ((android & ~kKnown) != 0) return false;
  int result = 0;
  if ((android & kAndroidMsgOob) != 0) result |= MSG_OOB;
  if ((android & kAndroidMsgPeek) != 0) result |= MSG_PEEK;
  if ((android & kAndroidMsgDontRoute) != 0) result |= MSG_DONTROUTE;
  if ((android & kAndroidMsgDontWait) != 0) result |= MSG_DONTWAIT;
  if ((android & kAndroidMsgEor) != 0) result |= MSG_EOR;
  if ((android & kAndroidMsgWaitAll) != 0) result |= MSG_WAITALL;
  *host = result;
  return true;
}

bool TranslatePollEvents(int16_t android, short* host) {
  constexpr int16_t kKnown = kAndroidPollIn | kAndroidPollPri |
                             kAndroidPollOut | kAndroidPollRdNorm |
                             kAndroidPollRdBand | kAndroidPollWrNorm |
                             kAndroidPollWrBand | kAndroidPollErr |
                             kAndroidPollHup | kAndroidPollNval;
  if ((android & ~kKnown) != 0) return false;
  short result = 0;
  if ((android & kAndroidPollIn) != 0) result |= POLLIN;
  if ((android & kAndroidPollPri) != 0) result |= POLLPRI;
  if ((android & kAndroidPollOut) != 0) result |= POLLOUT;
#ifdef POLLRDNORM
  if ((android & kAndroidPollRdNorm) != 0) result |= POLLRDNORM;
#endif
#ifdef POLLRDBAND
  if ((android & kAndroidPollRdBand) != 0) result |= POLLRDBAND;
#endif
#ifdef POLLWRNORM
  if ((android & kAndroidPollWrNorm) != 0) result |= POLLWRNORM;
#endif
#ifdef POLLWRBAND
  if ((android & kAndroidPollWrBand) != 0) result |= POLLWRBAND;
#endif
  *host = result;
  return true;
}

int16_t TranslatePollResults(short host, int16_t requested) {
  int16_t result = 0;
  if ((host & POLLIN) != 0) result |= kAndroidPollIn;
  if ((host & POLLPRI) != 0) result |= kAndroidPollPri;
  if ((host & POLLOUT) != 0 && (requested & kAndroidPollOut) != 0) {
    result |= kAndroidPollOut;
  }
  if ((host & POLLERR) != 0) result |= kAndroidPollErr;
  if ((host & POLLHUP) != 0) result |= kAndroidPollHup;
  if ((host & POLLNVAL) != 0) result |= kAndroidPollNval;
#ifdef POLLRDNORM
  if ((host & POLLRDNORM) != 0) result |= kAndroidPollRdNorm;
#endif
#ifdef POLLRDBAND
  if ((host & POLLRDBAND) != 0) result |= kAndroidPollRdBand;
#endif
#ifdef POLLWRNORM
  if ((host & POLLWRNORM) != 0 && (requested & kAndroidPollWrNorm) != 0) {
    result |= kAndroidPollWrNorm;
  }
#endif
#ifdef POLLWRBAND
  if ((host & POLLWRBAND) != 0) result |= kAndroidPollWrBand;
#endif
  return result;
}

int TranslateOutputMessageFlags(int host) {
  int result = 0;
  if ((host & MSG_OOB) != 0) result |= kAndroidMsgOob;
  if ((host & MSG_EOR) != 0) result |= kAndroidMsgEor;
  if ((host & MSG_TRUNC) != 0) result |= kAndroidMsgTrunc;
  if ((host & MSG_CTRUNC) != 0) result |= kAndroidMsgCtrunc;
  return result;
}

constexpr size_t kMaxIovecs = 1024;

bool BuildHostIovecs(const AimAndroidIovec* android, uint64_t count,
                     std::array<iovec, kMaxIovecs>* host) {
  if (count > kMaxIovecs || (count != 0 && android == nullptr)) return false;
  uint64_t total = 0;
  for (uint64_t index = 0; index < count; ++index) {
    const uint64_t length = android[index].iov_len;
    if (length > static_cast<uint64_t>(std::numeric_limits<ssize_t>::max()) ||
        total > static_cast<uint64_t>(std::numeric_limits<ssize_t>::max()) -
                    length) {
      return false;
    }
    total += length;
    (*host)[index] =
        iovec{android[index].iov_base, static_cast<size_t>(length)};
  }
  return true;
}

bool ToHostAddress(const void* android_address, uint32_t length,
                   sockaddr_storage* storage, socklen_t* host_length) {
  if (android_address == nullptr || length < sizeof(uint16_t)) return false;
  uint16_t family = 0;
  std::memcpy(&family, android_address, sizeof(family));
  std::memset(storage, 0, sizeof(*storage));
  if (family == kAndroidAfInet && length >= sizeof(AndroidSockaddrIn)) {
    AndroidSockaddrIn android{};
    std::memcpy(&android, android_address, sizeof(android));
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
    std::memcpy(&android, android_address, sizeof(android));
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

int32_t AddressFailure(const void* address, uint32_t length) {
  if (address == nullptr) return 14;
  if (length < sizeof(uint16_t)) return 22;
  uint16_t family = 0;
  std::memcpy(&family, address, sizeof(family));
  if (family == kAndroidAfInet || family == kAndroidAfInet6) return 22;
  return 97;
}

int FromHostAddress(const sockaddr* host, socklen_t host_length,
                    void* android_address, uint32_t* android_length) {
  if (android_length == nullptr) return Fail(14, -1);
  if (host->sa_family == AF_INET && host_length >= sizeof(sockaddr_in)) {
    const auto* source = reinterpret_cast<const sockaddr_in*>(host);
    AndroidSockaddrIn result{};
    result.family = kAndroidAfInet;
    result.port = source->sin_port;
    result.address = source->sin_addr.s_addr;
    const uint32_t capacity = *android_length;
    *android_length = sizeof(result);
    if (android_address != nullptr && capacity != 0) {
      std::memcpy(android_address, &result, std::min<uint32_t>(capacity, sizeof(result)));
    }
    return 0;
  }
  if (host->sa_family == AF_INET6 && host_length >= sizeof(sockaddr_in6)) {
    const auto* source = reinterpret_cast<const sockaddr_in6*>(host);
    AndroidSockaddrIn6 result{};
    result.family = kAndroidAfInet6;
    result.port = source->sin6_port;
    result.flowinfo = source->sin6_flowinfo;
    std::memcpy(result.address, &source->sin6_addr, sizeof(result.address));
    result.scope_id = source->sin6_scope_id;
    const uint32_t capacity = *android_length;
    *android_length = sizeof(result);
    if (android_address != nullptr && capacity != 0) {
      std::memcpy(android_address, &result, std::min<uint32_t>(capacity, sizeof(result)));
    }
    return 0;
  }
  return Fail(97, -1);
}

bool TranslateOption(int android_level, int android_option, int* host_level,
                     int* host_option) {
  if (android_level == kAndroidSolSocket) {
    *host_level = SOL_SOCKET;
    switch (android_option) {
      case kAndroidSoReuseaddr: *host_option = SO_REUSEADDR; return true;
      case kAndroidSoError: *host_option = SO_ERROR; return true;
      case kAndroidSoType: *host_option = SO_TYPE; return true;
      case kAndroidSoSndbuf: *host_option = SO_SNDBUF; return true;
      case kAndroidSoRcvbuf: *host_option = SO_RCVBUF; return true;
      case kAndroidSoKeepalive: *host_option = SO_KEEPALIVE; return true;
      default: return false;
    }
  }
  if (android_level == IPPROTO_TCP && android_option == 1) {
    *host_level = IPPROTO_TCP;
    *host_option = TCP_NODELAY;
    return true;
  }
  return false;
}

template <typename Operation>
int WithFd(int token, Operation operation) {
  const int fd = Borrow(token);
  if (fd == -1) return -1;
  const int result = operation(fd);
  const int operation_errno = errno;
  ReleaseBorrow(fd);
  if (result == -1) return Fail(AndroidErrno(operation_errno), -1);
  return result;
}

template <typename Operation>
intptr_t WithFdSize(int token, Operation operation) {
  const int fd = Borrow(token);
  if (fd == -1) return -1;
  const ssize_t result = operation(fd);
  const int operation_errno = errno;
  ReleaseBorrow(fd);
  if (result == -1) return Fail(AndroidErrno(operation_errno), intptr_t{-1});
  return static_cast<intptr_t>(result);
}

}  // namespace

extern "C" int aim_bionic_socket_socket(int domain, int type,
                                                 int protocol) {
  PreserveErrno preserve;
  int host_domain = 0;
  int host_type = 0;
  int host_protocol = 0;
  bool nonblocking = false;
  if (!TranslateDomain(domain, &host_domain)) return Fail(97, -1);
  if (!TranslateType(type, &host_type, &nonblocking)) return Fail(94, -1);
  if (!TranslateProtocol(protocol, &host_protocol)) return Fail(93, -1);
  const int fd = socket(host_domain, host_type, host_protocol);
  if (fd == -1) return FailHost();
  if (SetDescriptorFlags(fd, nonblocking) == -1) {
    const int error = errno;
    (void)close(fd);
    return Fail(AndroidErrno(error), -1);
  }
  return Install(fd);
}

extern "C" int aim_bionic_socket_socketpair(int domain, int type,
                                                     int protocol,
                                                     int32_t sockets[2]) {
  PreserveErrno preserve;
  if (sockets == nullptr) return Fail(14, -1);
  int host_domain = 0;
  int host_type = 0;
  bool nonblocking = false;
  if (!TranslateDomain(domain, &host_domain) || host_domain != AF_UNIX) {
    return Fail(97, -1);
  }
  if (!TranslateType(type, &host_type, &nonblocking)) return Fail(94, -1);
  if (protocol != 0) return Fail(93, -1);
  int pair[2]{-1, -1};
  if (socketpair(host_domain, host_type, 0, pair) == -1) return FailHost();
  if (SetDescriptorFlags(pair[0], nonblocking) == -1 ||
      SetDescriptorFlags(pair[1], nonblocking) == -1) {
    const int error = errno;
    (void)close(pair[0]);
    (void)close(pair[1]);
    return Fail(AndroidErrno(error), -1);
  }
  const int first = Install(pair[0]);
  if (first == -1) {
    (void)close(pair[1]);
    return -1;
  }
  const int second = Install(pair[1]);
  if (second == -1) {
    (void)aim_bionic_socket_close(first);
    return -1;
  }
  sockets[0] = first;
  sockets[1] = second;
  return 0;
}

extern "C" int aim_bionic_socket_close(int fd) {
  PreserveErrno preserve;
  size_t index = 0;
  uint32_t generation = 0;
  if (!Decode(fd, &index, &generation)) return Fail(9, -1);
  (void)pthread_mutex_lock(&g_lock);
  const int host_fd = g_slots[index].host_fd;
  if (host_fd == -1 || g_slots[index].generation != generation) {
    (void)pthread_mutex_unlock(&g_lock);
    return Fail(9, -1);
  }
  g_slots[index] = Slot{};
  (void)pthread_mutex_unlock(&g_lock);
  if (close(host_fd) == -1) return FailHost();
  return 0;
}

extern "C" int aim_bionic_socket_bind(int fd, const void* address,
                                               uint32_t length) {
  PreserveErrno preserve;
  sockaddr_storage storage{};
  socklen_t host_length = 0;
  if (!ToHostAddress(address, length, &storage, &host_length)) {
    return Fail(AddressFailure(address, length), -1);
  }
  return WithFd(fd, [&](int host) {
    return bind(host, reinterpret_cast<const sockaddr*>(&storage), host_length);
  });
}

extern "C" int aim_bionic_socket_connect(int fd, const void* address,
                                                  uint32_t length) {
  PreserveErrno preserve;
  sockaddr_storage storage{};
  socklen_t host_length = 0;
  if (!ToHostAddress(address, length, &storage, &host_length)) {
    return Fail(AddressFailure(address, length), -1);
  }
  return WithFd(fd, [&](int host) {
    return connect(host, reinterpret_cast<const sockaddr*>(&storage), host_length);
  });
}

extern "C" int aim_bionic_socket_listen(int fd, int backlog) {
  PreserveErrno preserve;
  return WithFd(fd, [&](int host) { return listen(host, backlog); });
}

extern "C" int aim_bionic_socket_accept4(int fd, void* address,
                                                  uint32_t* length,
                                                  int flags) {
  PreserveErrno preserve;
  if ((flags & ~(kAndroidSockNonblock | kAndroidSockCloexec)) != 0) {
    return Fail(22, -1);
  }
  const int listener = Borrow(fd);
  if (listener == -1) return -1;
  sockaddr_storage storage{};
  socklen_t host_length = sizeof(storage);
  const int accepted = accept(listener, reinterpret_cast<sockaddr*>(&storage),
                              &host_length);
  const int accept_errno = errno;
  ReleaseBorrow(listener);
  if (accepted == -1) return Fail(AndroidErrno(accept_errno), -1);
  if (SetDescriptorFlags(accepted, (flags & kAndroidSockNonblock) != 0) == -1) {
    const int error = errno;
    (void)close(accepted);
    return Fail(AndroidErrno(error), -1);
  }
  if (address != nullptr || length != nullptr) {
    if (address == nullptr || length == nullptr ||
        FromHostAddress(reinterpret_cast<sockaddr*>(&storage), host_length,
                        address, length) == -1) {
      (void)close(accepted);
      return -1;
    }
  }
  return Install(accepted);
}

extern "C" int aim_bionic_socket_shutdown(int fd, int how) {
  PreserveErrno preserve;
  int host_how = 0;
  switch (how) {
    case 0: host_how = SHUT_RD; break;
    case 1: host_how = SHUT_WR; break;
    case 2: host_how = SHUT_RDWR; break;
    default: return Fail(22, -1);
  }
  return WithFd(fd, [&](int host) { return shutdown(host, host_how); });
}

namespace {
int Name(int fd, void* address, uint32_t* length, bool peer) {
  PreserveErrno preserve;
  if (address == nullptr || length == nullptr) return Fail(14, -1);
  const int host = Borrow(fd);
  if (host == -1) return -1;
  sockaddr_storage storage{};
  socklen_t host_length = sizeof(storage);
  const int result = peer
      ? getpeername(host, reinterpret_cast<sockaddr*>(&storage), &host_length)
      : getsockname(host, reinterpret_cast<sockaddr*>(&storage), &host_length);
  const int operation_errno = errno;
  ReleaseBorrow(host);
  if (result == -1) return Fail(AndroidErrno(operation_errno), -1);
  return FromHostAddress(reinterpret_cast<sockaddr*>(&storage), host_length,
                         address, length);
}
}  // namespace

extern "C" int aim_bionic_socket_getsockname(int fd, void* address,
                                                      uint32_t* length) {
  return Name(fd, address, length, false);
}

extern "C" int aim_bionic_socket_getpeername(int fd, void* address,
                                                      uint32_t* length) {
  return Name(fd, address, length, true);
}

extern "C" int aim_bionic_socket_getsockopt(int fd, int level,
                                                     int option, void* value,
                                                     uint32_t* length) {
  PreserveErrno preserve;
  if (value == nullptr || length == nullptr) return Fail(14, -1);
  int host_level = 0;
  int host_option = 0;
  if (!TranslateOption(level, option, &host_level, &host_option)) {
    return Fail(92, -1);
  }
  if (level == kAndroidSolSocket && option == kAndroidSoError) {
    if (*length < sizeof(int)) return Fail(22, -1);
    int host_error = 0;
    socklen_t host_length = sizeof(host_error);
    const int result = WithFd(fd, [&](int host) {
      return getsockopt(host, host_level, host_option, &host_error,
                        &host_length);
    });
    if (result == 0) {
      const int32_t android_error = AndroidErrno(host_error);
      std::memcpy(value, &android_error, sizeof(android_error));
      *length = sizeof(android_error);
    }
    return result;
  }
  socklen_t host_length = *length;
  const int result = WithFd(fd, [&](int host) {
    return getsockopt(host, host_level, host_option, value, &host_length);
  });
  if (result == 0) *length = host_length;
  return result;
}

extern "C" int aim_bionic_socket_setsockopt(int fd, int level,
                                                     int option,
                                                     const void* value,
                                                     uint32_t length) {
  PreserveErrno preserve;
  if (value == nullptr) return Fail(14, -1);
  int host_level = 0;
  int host_option = 0;
  if (!TranslateOption(level, option, &host_level, &host_option)) {
    return Fail(92, -1);
  }
  return WithFd(fd, [&](int host) {
    return setsockopt(host, host_level, host_option, value, length);
  });
}

extern "C" intptr_t aim_bionic_socket_send(int fd, const void* buffer,
                                                    size_t length, int flags) {
  PreserveErrno preserve;
  int host_flags = 0;
  if (!TranslateFlags(flags, &host_flags)) return Fail(95, intptr_t{-1});
  return WithFdSize(fd, [&](int host) { return send(host, buffer, length, host_flags); });
}

extern "C" intptr_t aim_bionic_socket_recv(int fd, void* buffer,
                                                    size_t length, int flags) {
  PreserveErrno preserve;
  int host_flags = 0;
  if (!TranslateFlags(flags, &host_flags)) return Fail(95, intptr_t{-1});
  return WithFdSize(fd, [&](int host) { return recv(host, buffer, length, host_flags); });
}

extern "C" intptr_t aim_bionic_socket_sendto(
    int fd, const void* buffer, size_t length, int flags, const void* address,
    uint32_t address_length) {
  PreserveErrno preserve;
  int host_flags = 0;
  if (!TranslateFlags(flags, &host_flags)) return Fail(95, intptr_t{-1});
  sockaddr_storage storage{};
  socklen_t host_length = 0;
  if (address == nullptr && address_length == 0) {
    return WithFdSize(fd, [&](int host) {
      return sendto(host, buffer, length, host_flags, nullptr, 0);
    });
  }
  if (!ToHostAddress(address, address_length, &storage, &host_length)) {
    return Fail(AddressFailure(address, address_length), intptr_t{-1});
  }
  return WithFdSize(fd, [&](int host) {
    return sendto(host, buffer, length, host_flags,
                  reinterpret_cast<const sockaddr*>(&storage), host_length);
  });
}

extern "C" intptr_t aim_bionic_socket_recvfrom(
    int fd, void* buffer, size_t length, int flags, void* address,
    uint32_t* address_length) {
  PreserveErrno preserve;
  int host_flags = 0;
  if (!TranslateFlags(flags, &host_flags)) return Fail(95, intptr_t{-1});
  const int host = Borrow(fd);
  if (host == -1) return -1;
  sockaddr_storage storage{};
  socklen_t host_length = sizeof(storage);
  const ssize_t result = recvfrom(host, buffer, length, host_flags,
                                  reinterpret_cast<sockaddr*>(&storage),
                                  &host_length);
  const int operation_errno = errno;
  ReleaseBorrow(host);
  if (result == -1) return Fail(AndroidErrno(operation_errno), intptr_t{-1});
  if (address != nullptr || address_length != nullptr) {
    if (address == nullptr || address_length == nullptr ||
        FromHostAddress(reinterpret_cast<sockaddr*>(&storage), host_length,
                        address, address_length) == -1) {
      return -1;
    }
  }
  return result;
}

extern "C" int aim_bionic_socket_poll(
    AimAndroidPollfd* descriptors, uint32_t count, int timeout_ms) {
  PreserveErrno preserve;
  if (count > kMaxSockets) return Fail(22, -1);
  if (count != 0 && descriptors == nullptr) return Fail(14, -1);

  std::array<pollfd, kMaxSockets> host{};
  std::array<int, kMaxSockets> borrowed{};
  borrowed.fill(-1);
  int invalid_count = 0;
  for (uint32_t index = 0; index < count; ++index) {
    descriptors[index].revents = 0;
    short host_events = 0;
    if (!TranslatePollEvents(descriptors[index].events, &host_events)) {
      for (int descriptor : borrowed) {
        if (descriptor != -1) ReleaseBorrow(descriptor);
      }
      return Fail(22, -1);
    }
    host[index] = pollfd{-1, host_events, 0};
    if (descriptors[index].fd < 0) continue;
    int32_t borrow_error = 0;
    const int duplicate = BorrowRaw(descriptors[index].fd, &borrow_error);
    if (duplicate == -1) {
      if (borrow_error == 9) {
        descriptors[index].revents = kAndroidPollNval;
        ++invalid_count;
        continue;
      }
      for (int descriptor : borrowed) {
        if (descriptor != -1) ReleaseBorrow(descriptor);
      }
      return Fail(borrow_error, -1);
    }
    borrowed[index] = duplicate;
    host[index].fd = duplicate;
  }

  const int effective_timeout = invalid_count == 0 ? timeout_ms : 0;
  const int result = poll(host.data(), static_cast<nfds_t>(count),
                          effective_timeout);
  const int poll_errno = errno;
  if (result >= 0) {
    for (uint32_t index = 0; index < count; ++index) {
      if (borrowed[index] != -1) {
        descriptors[index].revents = TranslatePollResults(
            host[index].revents, descriptors[index].events);
      }
    }
  }
  for (int descriptor : borrowed) {
    if (descriptor != -1) ReleaseBorrow(descriptor);
  }
  if (result == -1) return Fail(AndroidErrno(poll_errno), -1);
  return result + invalid_count;
}

extern "C" int aim_bionic_socket_ppoll(
    AimAndroidPollfd* descriptors, uint32_t count,
    const AimAndroidTimespec* timeout, const void* android_sigset) {
  PreserveErrno preserve;
  if (android_sigset != nullptr) return Fail(95, -1);
  int timeout_ms = -1;
  if (timeout != nullptr) {
    if (timeout->tv_sec < 0 || timeout->tv_nsec < 0 ||
        timeout->tv_nsec >= 1000000000LL) {
      return Fail(22, -1);
    }
    constexpr int64_t kMillisecondsPerSecond = 1000;
    constexpr int64_t kNanosecondsPerMillisecond = 1000000;
    if (timeout->tv_sec >
        (std::numeric_limits<int>::max() - 1) / kMillisecondsPerSecond) {
      timeout_ms = std::numeric_limits<int>::max();
    } else {
      const int64_t rounded_nanoseconds =
          (timeout->tv_nsec + kNanosecondsPerMillisecond - 1) /
          kNanosecondsPerMillisecond;
      const int64_t milliseconds =
          timeout->tv_sec * kMillisecondsPerSecond + rounded_nanoseconds;
      timeout_ms = milliseconds > std::numeric_limits<int>::max()
                       ? std::numeric_limits<int>::max()
                       : static_cast<int>(milliseconds);
    }
  }
  return aim_bionic_socket_poll(descriptors, count, timeout_ms);
}

extern "C" intptr_t aim_bionic_socket_sendmsg(
    int fd, const AimAndroidMsghdr* message, int flags) {
  PreserveErrno preserve;
  if (message == nullptr) return Fail(14, intptr_t{-1});
  if (message->msg_control != nullptr || message->msg_controllen != 0) {
    return Fail(95, intptr_t{-1});
  }
  int host_flags = 0;
  if (!TranslateFlags(flags, &host_flags)) return Fail(95, intptr_t{-1});
  std::array<iovec, kMaxIovecs> iovecs{};
  if (!BuildHostIovecs(message->msg_iov, message->msg_iovlen, &iovecs)) {
    return Fail(22, intptr_t{-1});
  }
  sockaddr_storage storage{};
  socklen_t address_length = 0;
  sockaddr* address = nullptr;
  if (message->msg_name != nullptr || message->msg_namelen != 0) {
    if (!ToHostAddress(message->msg_name, message->msg_namelen, &storage,
                       &address_length)) {
      return Fail(AddressFailure(message->msg_name, message->msg_namelen),
                  intptr_t{-1});
    }
    address = reinterpret_cast<sockaddr*>(&storage);
  }
  msghdr host_message{};
  host_message.msg_name = address;
  host_message.msg_namelen = address_length;
  host_message.msg_iov = iovecs.data();
  host_message.msg_iovlen = static_cast<int>(message->msg_iovlen);
  host_message.msg_control = nullptr;
  host_message.msg_controllen = 0;
  return WithFdSize(fd, [&](int host) {
    return sendmsg(host, &host_message, host_flags);
  });
}

extern "C" intptr_t aim_bionic_socket_recvmsg(
    int fd, AimAndroidMsghdr* message, int flags) {
  PreserveErrno preserve;
  if (message == nullptr) return Fail(14, intptr_t{-1});
  if (message->msg_control != nullptr || message->msg_controllen != 0) {
    return Fail(95, intptr_t{-1});
  }
  int host_flags = 0;
  if (!TranslateFlags(flags, &host_flags)) return Fail(95, intptr_t{-1});
  std::array<iovec, kMaxIovecs> iovecs{};
  if (!BuildHostIovecs(message->msg_iov, message->msg_iovlen, &iovecs)) {
    return Fail(22, intptr_t{-1});
  }
  sockaddr_storage storage{};
  socklen_t address_length = sizeof(storage);
  const bool wants_address =
      message->msg_name != nullptr || message->msg_namelen != 0;
  if (wants_address && message->msg_name == nullptr) {
    return Fail(14, intptr_t{-1});
  }
  msghdr host_message{};
  host_message.msg_name = wants_address ? &storage : nullptr;
  host_message.msg_namelen = wants_address ? address_length : 0;
  host_message.msg_iov = iovecs.data();
  host_message.msg_iovlen = static_cast<int>(message->msg_iovlen);
  host_message.msg_control = nullptr;
  host_message.msg_controllen = 0;
  const int host = Borrow(fd);
  if (host == -1) return -1;
  const ssize_t result = recvmsg(host, &host_message, host_flags);
  const int operation_errno = errno;
  ReleaseBorrow(host);
  if (result == -1) return Fail(AndroidErrno(operation_errno), intptr_t{-1});
  if (wants_address) {
    uint32_t guest_length = message->msg_namelen;
    if (FromHostAddress(reinterpret_cast<sockaddr*>(&storage),
                        host_message.msg_namelen, message->msg_name,
                        &guest_length) == -1) {
      return -1;
    }
    message->msg_namelen = guest_length;
  }
  message->msg_controllen = 0;
  message->msg_flags = TranslateOutputMessageFlags(host_message.msg_flags);
  return static_cast<intptr_t>(result);
}

extern "C" AimBionicSocketFunction aim_bionic_socket_resolve(
    const char* soname, const char* symbol, const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::strcmp(soname, "libc.so") != 0 || std::strcmp(version, "LIBC") != 0) {
    return nullptr;
  }
#define SOCKET_SYMBOL(name)                                                    \
  if (std::strcmp(symbol, #name) == 0)                                         \
    return reinterpret_cast<AimBionicSocketFunction>(                    \
        aim_bionic_socket_##name)
  SOCKET_SYMBOL(socket);
  SOCKET_SYMBOL(socketpair);
  SOCKET_SYMBOL(close);
  SOCKET_SYMBOL(bind);
  SOCKET_SYMBOL(connect);
  SOCKET_SYMBOL(listen);
  SOCKET_SYMBOL(accept4);
  SOCKET_SYMBOL(shutdown);
  SOCKET_SYMBOL(getsockname);
  SOCKET_SYMBOL(getpeername);
  SOCKET_SYMBOL(getsockopt);
  SOCKET_SYMBOL(setsockopt);
  SOCKET_SYMBOL(send);
  SOCKET_SYMBOL(recv);
  SOCKET_SYMBOL(sendto);
  SOCKET_SYMBOL(recvfrom);
  SOCKET_SYMBOL(poll);
  SOCKET_SYMBOL(ppoll);
  SOCKET_SYMBOL(sendmsg);
  SOCKET_SYMBOL(recvmsg);
#undef SOCKET_SYMBOL
  return nullptr;
}

extern "C" const char* aim_bionic_socket_capability(
    const char* capability) {
  if (capability == nullptr) return "invalid-capability";
  if (std::strcmp(capability, "virtual-fd-table") == 0 ||
      std::strcmp(capability, "ipv4-ipv6-address-translation") == 0 ||
      std::strcmp(capability, "tcp-udp-loopback") == 0 ||
      std::strcmp(capability, "poll-ppoll-null-sigmask") == 0 ||
      std::strcmp(capability, "scatter-gather-no-control") == 0 ||
      std::strcmp(capability, "borrowed-duplicate-close-safety") == 0) {
    return "supported";
  }
  return "unsupported";
}

extern "C" void aim_bionic_socket_reset_for_test() {
  PreserveErrno preserve;
  std::array<int, kMaxSockets> descriptors{};
  descriptors.fill(-1);
  (void)pthread_mutex_lock(&g_lock);
  for (size_t index = 0; index < g_slots.size(); ++index) {
    descriptors[index] = g_slots[index].host_fd;
    g_slots[index] = Slot{};
  }
  (void)pthread_mutex_unlock(&g_lock);
  for (int descriptor : descriptors) {
    if (descriptor != -1) (void)close(descriptor);
  }
}

extern "C" size_t aim_bionic_socket_live_tokens_for_test() {
  PreserveErrno preserve;
  size_t count = 0;
  (void)pthread_mutex_lock(&g_lock);
  for (const Slot& slot : g_slots) count += slot.host_fd != -1 ? 1 : 0;
  (void)pthread_mutex_unlock(&g_lock);
  return count;
}

extern "C" size_t aim_bionic_socket_borrowed_descriptors_for_test() {
  PreserveErrno preserve;
  (void)pthread_mutex_lock(&g_lock);
  const size_t count = g_borrowed_descriptors;
  (void)pthread_mutex_unlock(&g_lock);
  return count;
}
