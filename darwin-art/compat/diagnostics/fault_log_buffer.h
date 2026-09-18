#pragma once

#include <cstddef>
#include <cstdint>

namespace darwin_art::diagnostics {

// Fixed-capacity writer for the native fault handler. The caller owns storage;
// this type performs no allocation, locking, environment lookup or formatting
// through libc. Capacities of at least two bytes always finish with a newline.
class FaultLogBuffer final {
 public:
  static constexpr std::size_t kMinimumCapacity = 2;

  struct Result {
    const char* data;
    std::size_t size;
  };

  FaultLogBuffer(char* data, std::size_t capacity)
      : data_(data), capacity_(capacity), size_(0), truncated_(false),
        finished_(false) {}

  void AppendLiteral(const char* literal) {
    if (literal == nullptr) {
      truncated_ = true;
      return;
    }
    while (*literal != '\0') {
      if (data_ == nullptr || capacity_ < kMinimumCapacity ||
          size_ >= capacity_ - 1 || finished_) {
        truncated_ = true;
      } else {
        data_[size_++] = *literal;
      }
      ++literal;
    }
  }

  void AppendHex(std::uintptr_t value) {
    char digits[2 + sizeof(std::uintptr_t) * 2 + 1];
    std::size_t count = 0;
    digits[count++] = '0';
    digits[count++] = 'x';
    bool significant = false;
    for (std::size_t nibble = sizeof(std::uintptr_t) * 2; nibble != 0;
         --nibble) {
      const unsigned shift = static_cast<unsigned>((nibble - 1) * 4);
      const unsigned value_nibble =
          static_cast<unsigned>((value >> shift) & static_cast<std::uintptr_t>(0xf));
      if (value_nibble != 0 || significant || nibble == 1) {
        significant = true;
        digits[count++] = "0123456789abcdef"[value_nibble];
      }
    }
    digits[count] = '\0';
    AppendLiteral(digits);
  }

  [[nodiscard]] bool truncated() const { return truncated_; }
  [[nodiscard]] std::size_t size() const { return size_; }

  // A valid (capacity >= 2, non-null storage) result always reserves and
  // includes its final newline, even when all payload space was exhausted.
  Result Finish() {
    if (!finished_) {
      if (data_ == nullptr || capacity_ < kMinimumCapacity) {
        truncated_ = true;
        size_ = 0;
      } else {
        if (size_ >= capacity_) size_ = capacity_ - 1;
        data_[size_++] = '\n';
      }
      finished_ = true;
    }
    return {data_, size_};
  }

 private:
  char* data_;
  std::size_t capacity_;
  std::size_t size_;
  bool truncated_;
  bool finished_;
};

static_assert(FaultLogBuffer::kMinimumCapacity >= 2,
              "fault log must reserve a trailing newline");

}  // namespace darwin_art::diagnostics
