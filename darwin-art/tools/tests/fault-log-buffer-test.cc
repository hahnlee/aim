#include "compat/diagnostics/fault_log_buffer.h"

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <limits>

using darwin_art::diagnostics::FaultLogBuffer;

namespace {
struct GuardedBuffer {
  unsigned char before[16];
  char bytes[2048];
  unsigned char after[16];

  GuardedBuffer() {
    std::memset(before, 0xa5, sizeof(before));
    std::memset(after, 0x5a, sizeof(after));
  }

  void CheckGuards() const {
    for (unsigned char byte : before) assert(byte == 0xa5);
    for (unsigned char byte : after) assert(byte == 0x5a);
  }
};
}

static void TestTinyCapacityAndNewline() {
  unsigned char before = 0xa5;
  char bytes[2] = {};
  unsigned char after = 0x5a;
  FaultLogBuffer buffer(bytes, sizeof(bytes));
  buffer.AppendLiteral("payload");
  buffer.AppendHex(0x1234);
  const auto result = buffer.Finish();
  assert(result.data == bytes && result.size == sizeof(bytes));
  assert(bytes[1] == '\n' && buffer.truncated());
  assert(before == 0xa5 && after == 0x5a);
  assert(buffer.Finish().size == result.size);
}

static void TestLongLabelAndHexOverflow() {
  GuardedBuffer guarded;
  FaultLogBuffer buffer(guarded.bytes, sizeof(guarded.bytes));
  for (int i = 0; i != 600; ++i) buffer.AppendLiteral("fault-label/");
  buffer.AppendHex(std::numeric_limits<std::uintptr_t>::max());
  const auto result = buffer.Finish();
  assert(result.size == sizeof(guarded.bytes));
  assert(result.data[result.size - 1] == '\n');
  assert(buffer.truncated());
  guarded.CheckGuards();
}

static void TestRepeatedAppendAndHex() {
  char bytes[64] = {};
  FaultLogBuffer buffer(bytes, sizeof(bytes));
  buffer.AppendLiteral("pc=");
  buffer.AppendHex(static_cast<std::uintptr_t>(0));
  buffer.AppendLiteral(" ");
  buffer.AppendHex(static_cast<std::uintptr_t>(0xfeed));
  const auto result = buffer.Finish();
  assert(!buffer.truncated());
  assert(result.size == std::strlen("pc=0x0 0xfeed\n"));
  assert(std::memcmp(result.data, "pc=0x0 0xfeed\n", result.size) == 0);
}

static void TestInvalidSmallCapacityIsSafe() {
  char byte = 'x';
  FaultLogBuffer zero(nullptr, 0);
  zero.AppendLiteral("ignored");
  assert(zero.Finish().size == 0 && zero.truncated());
  FaultLogBuffer one(&byte, 1);
  one.AppendLiteral("ignored");
  assert(one.Finish().size == 0 && one.truncated() && byte == 'x');
}

int main() {
  TestTinyCapacityAndNewline();
  TestLongLabelAndHexOverflow();
  TestRepeatedAppendAndHex();
  TestInvalidSmallCapacityIsSafe();
  std::puts("fault log buffer: bounded literals/hex, canaries, truncation, newline PASS");
}
