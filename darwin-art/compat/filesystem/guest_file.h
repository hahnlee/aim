#pragma once
#include <memory>
#include <string>
#include <string_view>

namespace darwin_art::filesystem {
// Holds one guest open-file description for its full lifetime. Rust owns the
// backing resource; no host FD or path-based reopen is exposed to callers.
class GuestFile final {
 public:
  static std::unique_ptr<GuestFile> Open(const std::string& path, bool read_write = false);
  ~GuestFile();
  GuestFile(const GuestFile&) = delete;
  GuestFile& operator=(const GuestFile&) = delete;
  bool ReadAll(std::string* output);
  bool WriteAtStart(std::string_view bytes); // Does not truncate the file.

 private:
  GuestFile() = default;
  int fd_ = -1;
};
}  // namespace darwin_art::filesystem
