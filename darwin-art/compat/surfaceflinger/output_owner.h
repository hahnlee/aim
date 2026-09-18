#pragma once

#include "output_lifetime_protocol.h"

#include <memory>

namespace darwin_art::surfaceflinger {

// Owned exclusively by the DarwinArtSurface lifetime. Closing this CLOEXEC
// connection retires the remote output even on process death. It must never
// appear in a renderer/GPU child descriptor handoff.
class OutputOwner {
 public:
  static std::unique_ptr<OutputOwner> Register(const char* endpoint,
                                              OutputRequest backing);
  ~OutputOwner();
  OutputOwner(const OutputOwner&) = delete;
  OutputOwner& operator=(const OutputOwner&) = delete;
  bool Replace(OutputRequest backing);
  void Retire();
  // Resize must distinguish a rejected update from lost ownership. A closed
  // connection cannot authorize publication of the former local backing.
  bool locally_open() const { return descriptor_ >= 0; }
  OutputToken token() const { return token_; }
  uint64_t generation() const { return generation_; }

 private:
  explicit OutputOwner(int descriptor) : descriptor_(descriptor) {}
  bool Exchange(OutputRequest request);
  int descriptor_ = -1;
  OutputToken token_{};
  uint64_t generation_ = 0;
};

}  // namespace darwin_art::surfaceflinger
