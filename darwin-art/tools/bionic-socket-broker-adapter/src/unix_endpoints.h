#pragma once
#include "unix_address.h"
#include <map>
#include <mutex>
#include <string>
#include <sys/socket.h>
#include <sys/un.h>

namespace darwin_art::socket {

// Trusted runtime installation only. The owner of each host endpoint must
// protect its containing directory and enforce service peer credentials.
// This registry does not confer permission to mutate service state.
class UnixEndpoints {
 public:
  bool Install(const UnixAddress& address, const std::string& host) {
    if (address.kind == UnixAddressKind::Unnamed ||
        (address.kind == UnixAddressKind::Pathname &&
         (address.name.empty() || address.name[0] != '/')) ||
        host.empty() || host[0] != '/' || host.find('\0') != std::string::npos ||
        host.size() >= sizeof(sockaddr_un::sun_path)) return false;
    std::lock_guard lock(mutex_);
    return endpoints_.emplace(Key(address.kind, address.name), host).second;
  }

  std::optional<sockaddr_un> Resolve(const UnixAddress& address) const {
    std::lock_guard lock(mutex_);
    const auto found = endpoints_.find(Key(address.kind, address.name));
    if (found == endpoints_.end()) return std::nullopt;
    sockaddr_un result{};
    result.sun_family = AF_UNIX;
    result.sun_len = static_cast<uint8_t>(offsetof(sockaddr_un, sun_path) +
                                           found->second.size() + 1);
    std::memcpy(result.sun_path, found->second.c_str(), found->second.size() + 1);
    return result;
  }
 private:
  using Key = std::pair<UnixAddressKind, std::vector<uint8_t>>;
  mutable std::mutex mutex_;
  std::map<Key, std::string> endpoints_;
};
}  // namespace darwin_art::socket
