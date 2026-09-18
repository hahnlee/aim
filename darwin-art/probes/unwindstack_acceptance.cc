#include <cstdio>
#include <string_view>
#include <unwindstack/AndroidUnwinder.h>

namespace {
bool CheckSequence(unwindstack::AndroidUnwinder& unwinder,
                   const unwindstack::AndroidUnwinderData& data,
                   const char* const* sequence, size_t size) {
  size_t next = 0;
  for (const auto& frame : data.frames) {
    const std::string_view name = static_cast<std::string_view>(frame.function_name);
    if (next < size && name.find(sequence[next]) != std::string_view::npos) ++next;
  }
  if (next == size) return true;
  for (const auto& frame : data.frames) {
    std::fprintf(stderr, "%s\n", unwinder.FormatFrame(frame).c_str());
  }
  return false;
}
}  // namespace

extern "C" bool darwin_art_unwindstack_check_local(const char* const* sequence,
                                                  size_t size) {
  unwindstack::AndroidLocalUnwinder unwinder;
  unwindstack::AndroidUnwinderData data;
  return unwinder.Unwind(data) && CheckSequence(unwinder, data, sequence, size);
}

extern "C" bool darwin_art_unwindstack_check_remote(int pid,
                                                   const char* const* sequence,
                                                   size_t size) {
  unwindstack::AndroidRemoteUnwinder unwinder(pid);
  unwindstack::AndroidUnwinderData data;
  return unwinder.Unwind(data) && CheckSequence(unwinder, data, sequence, size);
}
