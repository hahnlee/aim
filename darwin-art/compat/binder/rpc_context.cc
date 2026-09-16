#include "rpc_context.h"
#include "rpc_identity.h"

#include <binder/RpcSession.h>

#include <cerrno>
#include <cstddef>
#include <cstring>
#include <fcntl.h>
#include <string>
#include <sys/socket.h>
#include <sys/un.h>

namespace darwin_art::binder {
namespace {

using android::IBinder;
using android::RpcSession;
using android::binder::unique_fd;
using android::sp;

constexpr size_t kRpcWorkerThreads = 8;

unique_fd ConnectUnixSocket(const std::string& path) {
  sockaddr_un address{};
  if (path.empty() || path.size() >= sizeof(address.sun_path)) return {};

  unique_fd socket_fd(socket(AF_UNIX, SOCK_STREAM, /*protocol=*/0));
  if (!socket_fd.ok()) return {};
  if (fcntl(socket_fd.get(), F_SETFD, FD_CLOEXEC) != 0) return {};

  address.sun_family = AF_UNIX;
  std::memcpy(address.sun_path, path.c_str(), path.size() + 1);
  const socklen_t address_size = static_cast<socklen_t>(
      offsetof(sockaddr_un, sun_path) + path.size() + 1);
  if (connect(socket_fd.get(), reinterpret_cast<const sockaddr*>(&address),
              address_size) != 0) {
    return {};
  }
  return unique_fd(socket_fd.release());
}

}  // namespace

sp<IBinder> ConnectRpcContext(const char* path) {
  if (path == nullptr || *path == '\0') return nullptr;
  const std::string socket_path(path);
  sp<RpcSession> session = RpcSession::make();
  if (session == nullptr) return nullptr;
  session->setFileDescriptorTransportMode(
      RpcSession::FileDescriptorTransportMode::UNIX);
  session->setMaxIncomingThreads(kRpcWorkerThreads);
  unique_fd first_connection = ConnectUnixSocket(socket_path);
  if (!first_connection.ok() ||
      !darwin_art_binder_rpc_identity_bind(session.get(),
                                           first_connection.get())) {
    return nullptr;
  }
  if (session->setupPreconnectedClient(
          std::move(first_connection),
          [socket_path]() { return ConnectUnixSocket(socket_path); }) !=
      android::OK) {
    darwin_art_binder_rpc_identity_forget(session.get());
    return nullptr;
  }
  return session->getRootObject();
}

}  // namespace darwin_art::binder
