#pragma once

#include "aim_bionic_fs.h"
#include <memory>

namespace aim::filesystem {
// C++ owner of an opaque stream token; Rust owns the descriptor and entries.
// Never convertible to a host DIR*. Each returned entry is a caller-owned copy.
class GuestDirectory final {
 public:
  enum class Result { Entry, End, Error };
  static std::unique_ptr<GuestDirectory> Open(const char* path);
  ~GuestDirectory();
  GuestDirectory(const GuestDirectory&) = delete;
  GuestDirectory& operator=(const GuestDirectory&) = delete;
  Result Next(AimAndroidDirent* entry);

 private:
  GuestDirectory() = default;
  void* token_ = nullptr;
};
}  // namespace aim::filesystem
