#ifndef AIM_BIONIC_WIDE_STDIO_TEST_BACKEND_H_
#define AIM_BIONIC_WIDE_STDIO_TEST_BACKEND_H_

#include "aim_bionic_wide_stdio.h"

#include "aim_bionic_errno.h"

#include <condition_variable>
#include <cstdint>
#include <memory>
#include <mutex>
#include <unordered_map>
#include <utility>
#include <vector>

struct alignas(8) AimAndroidFile {
  unsigned char opaque[152];
};
static_assert(sizeof(AimAndroidFile) == 152);
static_assert(alignof(AimAndroidFile) == 8);

namespace wide_stdio_test {

constexpr int kEbadf = 9;

struct Stream {
  AimAndroidFile token{};
  std::mutex mutex;
  std::vector<uint8_t> input;
  std::vector<uint8_t> output;
  size_t position = 0;
  bool readable = true;
  bool writable = true;
  bool alive = true;
  bool error = false;
  bool eof = false;
  int orientation = 0;
};

struct Context {
  std::mutex table_mutex;
  std::unordered_map<AimAndroidFile*, std::unique_ptr<Stream>> streams;
  std::mutex gate_mutex;
  std::condition_variable gate_changed;
  bool block_next_read = false;
  bool read_is_blocked = false;
  bool release_read = false;

  Stream* Add(std::vector<uint8_t> input,
              bool readable = true,
              bool writable = true) {
    auto stream = std::make_unique<Stream>();
    stream->input = std::move(input);
    stream->readable = readable;
    stream->writable = writable;
    Stream* result = stream.get();
    std::lock_guard<std::mutex> lock(table_mutex);
    streams.emplace(&result->token, std::move(stream));
    return result;
  }

  bool Close(Stream* stream) {
    std::lock_guard<std::mutex> lock(stream->mutex);
    if (!stream->alive) return false;
    stream->alive = false;
    return aim_bionic_wide_stdio_forget(&stream->token) == 0;
  }

  bool Reset(Stream* stream, std::vector<uint8_t> input) {
    std::lock_guard<std::mutex> lock(stream->mutex);
    if (!stream->alive) return false;
    stream->input = std::move(input);
    stream->output.clear();
    stream->position = 0;
    stream->error = false;
    stream->eof = false;
    return aim_bionic_wide_stdio_reset(&stream->token) == 0;
  }

  bool Reopen(Stream* stream, std::vector<uint8_t> input) {
    std::lock_guard<std::mutex> lock(stream->mutex);
    if (stream->alive) return false;
    stream->input = std::move(input);
    stream->output.clear();
    stream->position = 0;
    stream->readable = true;
    stream->writable = true;
    stream->alive = true;
    stream->error = false;
    stream->eof = false;
    stream->orientation = 0;
    return true;
  }

  bool CloseAll() {
    std::vector<Stream*> snapshot;
    {
      std::lock_guard<std::mutex> lock(table_mutex);
      for (auto& [_, stream] : streams) snapshot.push_back(stream.get());
    }
    for (Stream* stream : snapshot) {
      std::lock_guard<std::mutex> lock(stream->mutex);
      if (!stream->alive) continue;
      stream->alive = false;
      if (aim_bionic_wide_stdio_forget(&stream->token) != 0) {
        return false;
      }
    }
    return true;
  }
};

inline int Acquire(void* value, AimAndroidFile* file, void** lease) {
  auto* context = static_cast<Context*>(value);
  if (context == nullptr || file == nullptr || lease == nullptr) {
    aim_bionic_errno_store(kEbadf);
    return -1;
  }
  Stream* stream = nullptr;
  {
    std::lock_guard<std::mutex> lock(context->table_mutex);
    auto found = context->streams.find(file);
    if (found == context->streams.end()) {
      aim_bionic_errno_store(kEbadf);
      return -1;
    }
    stream = found->second.get();
  }
  stream->mutex.lock();
  if (!stream->alive) {
    stream->mutex.unlock();
    aim_bionic_errno_store(kEbadf);
    return -1;
  }
  *lease = stream;
  return 0;
}

inline void Release(void*, void* lease) {
  static_cast<Stream*>(lease)->mutex.unlock();
}

inline int OrientWide(void*, void* lease) {
  auto* stream = static_cast<Stream*>(lease);
  if (stream->orientation == 0) stream->orientation = 1;
  return 0;
}

inline int ReadByte(void* value, void* lease, uint8_t* output) {
  auto* context = static_cast<Context*>(value);
  auto* stream = static_cast<Stream*>(lease);
  if (!stream->readable || output == nullptr) {
    stream->error = true;
    aim_bionic_errno_store(kEbadf);
    return -1;
  }
  {
    std::unique_lock<std::mutex> lock(context->gate_mutex);
    if (context->block_next_read) {
      context->block_next_read = false;
      context->read_is_blocked = true;
      context->gate_changed.notify_all();
      context->gate_changed.wait(lock, [&] { return context->release_read; });
    }
  }
  if (stream->position == stream->input.size()) {
    stream->eof = true;
    return 0;
  }
  *output = stream->input[stream->position++];
  return 1;
}

inline int WriteBytes(void*, void* lease, const uint8_t* bytes, size_t length) {
  auto* stream = static_cast<Stream*>(lease);
  if (!stream->writable || (length != 0 && bytes == nullptr)) {
    stream->error = true;
    aim_bionic_errno_store(kEbadf);
    return -1;
  }
  stream->output.insert(stream->output.end(), bytes, bytes + length);
  return 0;
}

inline void SetError(void*, void* lease) {
  static_cast<Stream*>(lease)->error = true;
}

inline void ClearErrorAndEof(void*, void* lease) {
  auto* stream = static_cast<Stream*>(lease);
  stream->error = false;
  stream->eof = false;
}

inline AimBionicWideStdioBackendV1 Operations(Context* context) {
  return {AIM_BIONIC_WIDE_STDIO_BACKEND_ABI,
          sizeof(AimBionicWideStdioBackendV1),
          context,
          Acquire,
          Release,
          OrientWide,
          ReadByte,
          WriteBytes,
          SetError,
          ClearErrorAndEof};
}

}  // namespace wide_stdio_test

#endif
