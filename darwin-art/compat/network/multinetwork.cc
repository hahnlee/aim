#include "network/multinetwork.h"

#include <atomic>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <sys/socket.h>
#include <unistd.h>

extern "C" void darwin_art_bionic_errno_store(int32_t android_errno);
extern "C" int darwin_art_bionic_errno_set_from_darwin(int darwin_errno);
extern "C" int darwin_art_bionic_fd_export_for_scm(int guest_fd);
extern "C" int darwin_art_bionic_socket_broker_close(int guest_fd);
extern "C" int darwin_art_bionic_socket_broker_res_nquery(
    uint64_t network, const char* name, int ns_class, int ns_type,
    uint32_t flags);
extern "C" int darwin_art_bionic_socket_broker_res_nresult(
    int fd, int* rcode, uint8_t* answer, size_t answer_length);

namespace {

constexpr uint64_t kNetworkUnspecified = 0;
constexpr uint32_t kNetworkHandleMagic = 0xcafed00dU;
constexpr int32_t kAndroidEinval = 22;
constexpr int32_t kAndroidEnonet = 64;
constexpr int32_t kAndroidEnotsup = 95;

// Android defines these as process bindings. Each Darwin ART application is a
// separate host process, so process-local atomics preserve that ownership.
std::atomic<uint64_t> g_process_network{kNetworkUnspecified};
std::atomic<uint64_t> g_dns_network{kNetworkUnspecified};

bool IsKnownNetwork(uint64_t network) {
  // The current ConnectivityService slice does not publish Network identities.
  // Zero clears a binding; accepting any nonzero handle would silently route a
  // disconnected Android Network over the host default path.
  return network == kNetworkUnspecified;
}

bool IsSyntacticallyValidNetwork(uint64_t network) {
  return network == kNetworkUnspecified ||
         static_cast<uint32_t>(network) == kNetworkHandleMagic;
}

int RejectUnknownNetwork(uint64_t network) {
  (void)IsSyntacticallyValidNetwork(network);
  darwin_art_bionic_errno_store(kAndroidEnonet);
  return -1;
}

extern "C" int AndroidSetProcessNetwork(uint64_t network) {
  if (!IsKnownNetwork(network)) return RejectUnknownNetwork(network);
  g_process_network.store(network, std::memory_order_release);
  return 0;
}

extern "C" int AndroidGetProcessNetwork(uint64_t* network) {
  if (network == nullptr) {
    darwin_art_bionic_errno_store(kAndroidEinval);
    return -1;
  }
  *network = g_process_network.load(std::memory_order_acquire);
  return 0;
}

extern "C" int AndroidSetProcessDns(uint64_t network) {
  if (!IsKnownNetwork(network)) return RejectUnknownNetwork(network);
  g_dns_network.store(network, std::memory_order_release);
  return 0;
}

extern "C" int AndroidGetProcessDns(uint64_t* network) {
  if (network == nullptr) {
    darwin_art_bionic_errno_store(kAndroidEinval);
    return -1;
  }
  const uint64_t process = g_process_network.load(std::memory_order_acquire);
  *network = process != kNetworkUnspecified
                 ? process
                 : g_dns_network.load(std::memory_order_acquire);
  return 0;
}

extern "C" int AndroidSetSocketNetwork(uint64_t network, int guest_fd) {
  if (!IsKnownNetwork(network)) return RejectUnknownNetwork(network);

  // Guest descriptors are central-broker tokens, never Darwin descriptors.
  // Export a leased duplicate and validate the actual object as a socket even
  // when clearing a binding, matching Android's EBADF/ENOTSOCK behavior.
  const int host_fd = darwin_art_bionic_fd_export_for_scm(guest_fd);
  if (host_fd < 0) return -1;
  int socket_type = 0;
  socklen_t length = sizeof(socket_type);
  const int result = getsockopt(host_fd, SOL_SOCKET, SO_TYPE, &socket_type,
                                &length);
  const int host_error = errno;
  (void)close(host_fd);
  if (result == 0) return 0;
  (void)darwin_art_bionic_errno_set_from_darwin(host_error);
  return -1;
}

extern "C" int AndroidResNquery(uint64_t network, const char* name,
                                  int ns_class, int ns_type, uint32_t flags) {
  if (!IsKnownNetwork(network)) return -kAndroidEnonet;
  return darwin_art_bionic_socket_broker_res_nquery(
      network, name, ns_class, ns_type, flags);
}

extern "C" int AndroidResNsend(uint64_t network, const uint8_t*, size_t,
                                uint32_t) {
  if (!IsKnownNetwork(network)) return -kAndroidEnonet;
  // The DNS owner does not yet implement raw wire-message submission. Report
  // the unsupported operation rather than manufacturing a readable result FD.
  return -kAndroidEnotsup;
}

extern "C" int AndroidResNresult(int fd, int* rcode, uint8_t* answer,
                                  size_t answer_length) {
  return darwin_art_bionic_socket_broker_res_nresult(
      fd, rcode, answer, answer_length);
}

extern "C" void AndroidResCancel(int fd) {
  // Closing the broker-owned query descriptor cancels delivery and retires the
  // one-shot result identity. No host descriptor escapes this owner.
  (void)darwin_art_bionic_socket_broker_close(fd);
}

bool SupportedVersion(const char* version) {
  return version == nullptr || *version == '\0' ||
         std::strcmp(version, "LIBANDROID") == 0;
}

}  // namespace

extern "C" void* darwin_art_android_multinetwork_symbol(
    const char* symbol, const char* version) {
  if (symbol == nullptr || !SupportedVersion(version)) return nullptr;
#define DARWIN_MULTINETWORK_SYMBOL(name, function) \
  if (std::strcmp(symbol, name) == 0)             \
    return reinterpret_cast<void*>(&function)
  DARWIN_MULTINETWORK_SYMBOL("android_setprocnetwork", AndroidSetProcessNetwork);
  DARWIN_MULTINETWORK_SYMBOL("android_getprocnetwork", AndroidGetProcessNetwork);
  DARWIN_MULTINETWORK_SYMBOL("android_setprocdns", AndroidSetProcessDns);
  DARWIN_MULTINETWORK_SYMBOL("android_getprocdns", AndroidGetProcessDns);
  DARWIN_MULTINETWORK_SYMBOL("android_setsocknetwork", AndroidSetSocketNetwork);
  DARWIN_MULTINETWORK_SYMBOL("android_res_nquery", AndroidResNquery);
  DARWIN_MULTINETWORK_SYMBOL("android_res_nsend", AndroidResNsend);
  DARWIN_MULTINETWORK_SYMBOL("android_res_nresult", AndroidResNresult);
  DARWIN_MULTINETWORK_SYMBOL("android_res_cancel", AndroidResCancel);
#undef DARWIN_MULTINETWORK_SYMBOL
  return nullptr;
}
