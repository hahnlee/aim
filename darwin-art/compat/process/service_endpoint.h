#pragma once
#include <cstddef>
#include <atomic>
#include <cstdint>
#include <cstring>
#include <utility>

extern "C" int32_t darwin_art_service_endpoint_open(
    const uint8_t*, size_t, uint64_t*, int32_t*);
extern "C" int32_t darwin_art_service_endpoint_close(uint64_t);
extern "C" int32_t darwin_art_service_endpoint_ready(uint64_t, uint32_t);
extern "C" int32_t darwin_art_service_endpoint_lost(uint64_t, uint32_t);

namespace darwin_art::process {
enum class ServiceReadiness : uint32_t { Binder = 1, Compositor = 2 };
enum class ReadinessPublication { Failed, Published, Unmanaged };
// Native code borrows the descriptor. Rust owns close, the socket inode and
// the lifetime lock; a pre-existing endpoint is never treated as our success.
class ServiceEndpoint {
 public:
  explicit ServiceEndpoint(const char* path) {
    if (path != nullptr) {
      darwin_art_service_endpoint_open(reinterpret_cast<const uint8_t*>(path),
                                      std::strlen(path), &owner_, &descriptor_);
    }
  }
  ServiceEndpoint(ServiceEndpoint&& other) noexcept
      : owner_(std::exchange(other.owner_, 0)),
        descriptor_(std::exchange(other.descriptor_, -1)),
        published_(other.published_.exchange(0)) {}
  ServiceEndpoint(const ServiceEndpoint&) = delete;
  ServiceEndpoint& operator=(const ServiceEndpoint&) = delete;
  ~ServiceEndpoint() {
    if (owner_ == 0) return;
    // Destruction of a published transport is also capability loss. Explicit
    // fatal paths report before closing; this covers unwinding/early returns.
    const auto bits = published_.exchange(0);
    if (bits != 0) darwin_art_service_endpoint_lost(owner_, bits);
    darwin_art_service_endpoint_close(owner_);
  }
  int descriptor() const { return descriptor_; }
  ReadinessPublication publish_ready(ServiceReadiness capability) const {
    switch (darwin_art_service_endpoint_ready(owner_, static_cast<uint32_t>(capability))) {
      case 0:
        published_.fetch_or(static_cast<uint32_t>(capability));
        return ReadinessPublication::Published;
      case 1: return ReadinessPublication::Unmanaged;
      default: return ReadinessPublication::Failed;
    }
  }
  ReadinessPublication report_lost(ServiceReadiness capability) const {
    const auto bits = static_cast<uint32_t>(capability);
    switch (darwin_art_service_endpoint_lost(owner_, bits)) {
      case 0:
        published_.fetch_and(~bits);
        return ReadinessPublication::Published;
      case 1: return ReadinessPublication::Unmanaged;
      default: return ReadinessPublication::Failed;
    }
  }
  explicit operator bool() const { return owner_ != 0; }
 private:
  uint64_t owner_ = 0;
  int32_t descriptor_ = -1;
  mutable std::atomic<uint32_t> published_{0};
};
}
