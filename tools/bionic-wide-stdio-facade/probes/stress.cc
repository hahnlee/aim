#include "backend.h"

#include "aim_bionic_locale.h"

#include <errno.h>

#include <array>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <thread>
#include <vector>

using wide_stdio_test::Context;
using wide_stdio_test::Operations;
using wide_stdio_test::Stream;

namespace {

bool Basic(Context& context) {
  Stream* input = context.Add({'A', 0xf0, 0x9f, 0x98, 0x80});
  errno = 31337;
  if (aim_bionic_getwc(&input->token) != 'A' || errno != 31337 ||
      aim_bionic_getwc(&input->token) != 0x1f600 ||
      aim_bionic_getwc(&input->token) != AIM_BIONIC_WEOF ||
      !input->eof || input->orientation != 1) {
    return false;
  }
  if (aim_bionic_ungetwc(0x1f642, &input->token) != 0x1f642 ||
      input->error || input->eof ||
      aim_bionic_ungetwc('X', &input->token) !=
          AIM_BIONIC_WEOF ||
      aim_bionic_getwc(&input->token) != 0x1f642) {
    return false;
  }

  Stream* output = context.Add({}, false, true);
  if (aim_bionic_fputwc(0x1f600, &output->token) != 0x1f600 ||
      aim_bionic_fputwc(0xd800, &output->token) != 0xd800 ||
      output->output !=
          std::vector<uint8_t>({0xf0, 0x9f, 0x98, 0x80, 0xed, 0xa0, 0x80})) {
    return false;
  }
  aim_bionic_errno_store(0);
  if (aim_bionic_fputwc(0x200000, &output->token) !=
          AIM_BIONIC_WEOF ||
      aim_bionic_errno_load() != 84 || !output->error) {
    return false;
  }

  Stream* invalid = context.Add({0xed, 0xa0, 0x80});
  aim_bionic_errno_store(0);
  if (aim_bionic_getwc(&invalid->token) != AIM_BIONIC_WEOF ||
      aim_bionic_errno_load() != 84 || !invalid->error) {
    return false;
  }

  // Pinned fgetwc returns plain WEOF when EOF terminates an incomplete
  // sequence: the byte backend owns EOF and Bionic neither synthesizes
  // EILSEQ nor sets the stream error bit in this branch.
  Stream* partial = context.Add({0xe2, 0x82});
  aim_bionic_errno_store(777);
  if (aim_bionic_getwc(&partial->token) != AIM_BIONIC_WEOF ||
      aim_bionic_errno_load() != 777 || partial->error ||
      !partial->eof) {
    return false;
  }

  Stream* pushback = context.Add({'Z'});
  if (aim_bionic_ungetwc('X', &pushback->token) != 'X' ||
      aim_bionic_fputwc('A', &pushback->token) != 'A' ||
      aim_bionic_getwc(&pushback->token) != 'Z') {
    return false;
  }


  Stream* reset = context.Add({0xf0});
  if (aim_bionic_getwc(&reset->token) != AIM_BIONIC_WEOF ||
      !context.Reset(reset, {'N'}) ||
      aim_bionic_getwc(&reset->token) != 'N') {
    return false;
  }
  return true;
}

bool Concurrent(Context& context) {
  constexpr size_t kCount = 8000;
  Stream* stream = context.Add(std::vector<uint8_t>(kCount, 'q'), true, false);
  std::atomic<size_t> observed{0};
  std::atomic<bool> failed{false};
  std::array<std::thread, 8> threads;
  for (auto& thread : threads) {
    thread = std::thread([&] {
      for (;;) {
        uint32_t value = aim_bionic_getwc(&stream->token);
        if (value == AIM_BIONIC_WEOF) return;
        if (value != 'q') failed = true;
        ++observed;
      }
    });
  }
  for (auto& thread : threads) thread.join();
  return !failed && observed == kCount && stream->position == kCount;
}

bool IndependentStreams(Context& context) {
  Stream* blocked = context.Add({'B'}, true, false);
  Stream* independent = context.Add({'I'}, true, false);
  {
    std::lock_guard<std::mutex> lock(context.gate_mutex);
    context.block_next_read = true;
    context.read_is_blocked = false;
    context.release_read = false;
  }
  uint32_t blocked_result = 0;
  std::thread first(
      [&] { blocked_result = aim_bionic_getwc(&blocked->token); });
  {
    std::unique_lock<std::mutex> lock(context.gate_mutex);
    context.gate_changed.wait(lock, [&] { return context.read_is_blocked; });
  }
  std::mutex done_mutex;
  std::condition_variable done_changed;
  bool done = false;
  uint32_t independent_result = 0;
  std::thread second([&] {
    independent_result = aim_bionic_getwc(&independent->token);
    std::lock_guard<std::mutex> lock(done_mutex);
    done = true;
    done_changed.notify_all();
  });
  bool completed_while_first_was_blocked = false;
  {
    std::unique_lock<std::mutex> lock(done_mutex);
    completed_while_first_was_blocked = done_changed.wait_for(
        lock, std::chrono::seconds(2), [&] { return done; });
  }
  {
    std::lock_guard<std::mutex> lock(context.gate_mutex);
    context.release_read = true;
    context.gate_changed.notify_all();
  }
  first.join();
  second.join();
  return completed_while_first_was_blocked && blocked_result == 'B' &&
         independent_result == 'I';
}

bool ResetRace(Context& context) {
  Stream* stream = context.Add({'R'}, true, false);
  {
    std::lock_guard<std::mutex> lock(context.gate_mutex);
    context.block_next_read = true;
    context.read_is_blocked = false;
    context.release_read = false;
  }
  uint32_t result = 0;
  std::thread reader([&] { result = aim_bionic_getwc(&stream->token); });
  {
    std::unique_lock<std::mutex> lock(context.gate_mutex);
    context.gate_changed.wait(lock, [&] { return context.read_is_blocked; });
  }
  bool reset = false;
  std::thread resetter([&] { reset = context.Reset(stream, {'N'}); });
  {
    std::lock_guard<std::mutex> lock(context.gate_mutex);
    context.release_read = true;
    context.gate_changed.notify_all();
  }
  reader.join();
  resetter.join();
  return result == 'R' && reset &&
         aim_bionic_getwc(&stream->token) == 'N';
}

bool CloseRace(Context& context) {
  Stream* stream = context.Add({'R'}, true, false);
  {
    std::lock_guard<std::mutex> lock(context.gate_mutex);
    context.block_next_read = true;
    context.read_is_blocked = false;
    context.release_read = false;
  }
  uint32_t result = 0;
  std::thread reader([&] { result = aim_bionic_getwc(&stream->token); });
  {
    std::unique_lock<std::mutex> lock(context.gate_mutex);
    context.gate_changed.wait(lock, [&] { return context.read_is_blocked; });
  }
  bool closed = false;
  std::thread closer([&] { closed = context.Close(stream); });
  {
    std::lock_guard<std::mutex> lock(context.gate_mutex);
    context.release_read = true;
    context.gate_changed.notify_all();
  }
  reader.join();
  closer.join();
  aim_bionic_errno_store(0);
  if (result != 'R' || !closed ||
      aim_bionic_getwc(&stream->token) != AIM_BIONIC_WEOF ||
      aim_bionic_errno_load() != 9) {
    return false;
  }

  // The central FILE owner may recycle the same 152-byte token only after
  // forget() has completed under its exclusive lease. No old mbstate or
  // pushback may survive that generation boundary.
  if (!context.Reopen(stream, {'S'}) ||
      aim_bionic_getwc(&stream->token) != 'S') {
    return false;
  }
  return true;
}

}  // namespace

int main() {
  if (aim_bionic_setlocale(0, "C.UTF-8") == nullptr) return 10;
  Context context;
  auto operations = Operations(&context);
  AimBionicWideStdioActivation* activation =
      aim_bionic_wide_stdio_install(&operations);
  if (activation == nullptr || !Basic(context) || !Concurrent(context) ||
      !IndependentStreams(context) || !ResetRace(context) ||
      !CloseRace(context) || !context.CloseAll() ||
      aim_bionic_wide_stdio_uninstall(&activation) != 0 ||
      activation != nullptr) {
    return 11;
  }
  std::puts("bionic-wide-stdio: PASS UTF-8+surrogate-asymmetry+partial-EOF+pushback "
            "threads=8 per-stream-parallel close/reset-race=serialized "
            "token-reuse=clean "
            "host-errno=preserved ASan+UBSan=clean");
  return 0;
}
