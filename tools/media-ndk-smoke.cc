// Standalone contract test linked with the runtime's NDK media object. The
// broker FD hooks below are test adapters only; custom-source calls use the
// same production extractor/decoder implementations as Android clients.
#include "../compat/darwin_android_media_ndk.h"
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <vector>
#include <fcntl.h>
#include <unistd.h>

extern "C" intptr_t aim_bionic_pread(int fd, void* p, size_t n, int64_t offset) {
  return pread(fd, p, n, offset);
}
extern "C" intptr_t aim_bionic_read(int fd, void* p, size_t n) { return read(fd, p, n); }
extern "C" int aim_bionic_open(const char* p, int flags, uint32_t mode) { return open(p, flags, mode); }
extern "C" int64_t aim_bionic_lseek(int fd, int64_t offset, int whence) { return lseek(fd, offset, whence); }
extern "C" int aim_bionic_close(int fd) { return close(fd); }

template<class T> T Api(const char* name) {
  auto symbol = aim_android_media_ndk_symbol(name);
  assert(symbol != nullptr);
  return reinterpret_cast<T>(symbol);
}
struct Source { std::vector<uint8_t> bytes; int closed = 0; };
static ssize_t ReadAt(void* raw, int64_t offset, void* output, size_t size) {
  auto& bytes = static_cast<Source*>(raw)->bytes;
  if (offset < 0 || uint64_t(offset) > bytes.size()) return -1;
  size = std::min(size, bytes.size() - size_t(offset));
  std::memcpy(output, bytes.data() + offset, size);
  return size;
}
static ssize_t GetSize(void* raw) { return static_cast<Source*>(raw)->bytes.size(); }
static void Close(void* raw) { ++static_cast<Source*>(raw)->closed; }
struct BufferInfo { int32_t offset, size; int64_t pts; uint32_t flags; };

int main(int argc, char** argv) {
  if (argc != 2) return 2;
  Source source;
  std::ifstream file(argv[1], std::ios::binary);
  source.bytes.assign(std::istreambuf_iterator<char>(file), {});
  assert(!source.bytes.empty());
  auto data = Api<void*(*)()>("AMediaDataSource_new")();
  Api<void(*)(void*,void*)>("AMediaDataSource_setUserdata")(data, &source);
  Api<void(*)(void*,decltype(&ReadAt))>("AMediaDataSource_setReadAt")(data, ReadAt);
  Api<void(*)(void*,decltype(&GetSize))>("AMediaDataSource_setGetSize")(data, GetSize);
  Api<void(*)(void*,decltype(&Close))>("AMediaDataSource_setClose")(data, Close);
  auto extractor = Api<void*(*)()>("AMediaExtractor_new")();
  assert(Api<int(*)(void*,void*)>("AMediaExtractor_setDataSourceCustom")(extractor, data) == 0);
  Api<void(*)(void*)>("AMediaDataSource_delete")(data);
  assert(source.closed == 1);
  assert(Api<size_t(*)(void*)>("AMediaExtractor_getTrackCount")(extractor) == 1);
  auto format = Api<void*(*)(void*,size_t)>("AMediaExtractor_getTrackFormat")(extractor, 0);
  assert(Api<int(*)(void*,size_t)>("AMediaExtractor_selectTrack")(extractor, 0) == 0);
  const char* mime = nullptr;
  assert(Api<bool(*)(void*,const char*,const char**)>("AMediaFormat_getString")(format, "mime", &mime));
  auto codec = Api<void*(*)(const char*)>("AMediaCodec_createDecoderByType")(mime);
  assert(codec);
  assert(Api<int(*)(void*,void*,void*,void*,uint32_t)>("AMediaCodec_configure")(codec, format, nullptr, nullptr, 0) == 0);
  assert(Api<int(*)(void*)>("AMediaCodec_start")(codec) == 0);
  auto dequeueInput = Api<ssize_t(*)(void*,int64_t)>("AMediaCodec_dequeueInputBuffer");
  auto dequeueOutput = Api<ssize_t(*)(void*,BufferInfo*,int64_t)>("AMediaCodec_dequeueOutputBuffer");
  bool input_eos = false, output_eos = false, format_seen = false;
  size_t frames = 0, format_changes = 0;
  int64_t previous_pts = -1;
  for (int iteration = 0; iteration < 10000 && !output_eos; ++iteration) {
    const auto input_index = input_eos ? -1 : dequeueInput(codec, 0);
    if (input_index >= 0) {
      assert(dequeueInput(codec, 0) == -1);
      size_t capacity = 0;
      auto* buffer = Api<uint8_t*(*)(void*,size_t,size_t*)>("AMediaCodec_getInputBuffer")(codec, input_index, &capacity);
      const auto count = Api<ssize_t(*)(void*,uint8_t*,size_t)>("AMediaExtractor_readSampleData")(extractor, buffer, capacity);
      const auto pts = Api<int64_t(*)(void*)>("AMediaExtractor_getSampleTime")(extractor);
      input_eos = count < 0;
      assert(Api<int(*)(void*,size_t,size_t,size_t,uint64_t,uint32_t)>("AMediaCodec_queueInputBuffer")(
          codec, input_index, 0, input_eos ? 0 : count, input_eos ? previous_pts : pts, input_eos ? 4 : 0) == 0);
      if (!input_eos) Api<bool(*)(void*)>("AMediaExtractor_advance")(extractor);
    }
    BufferInfo info{};
    auto index = dequeueOutput(codec, &info, 0);
    if (index == -2) {
      auto output_format = Api<void*(*)(void*)>("AMediaCodec_getOutputFormat")(codec);
      int width = 0, height = 0;
      assert(Api<bool(*)(void*,const char*,int32_t*)>("AMediaFormat_getInt32")(output_format, "width", &width));
      assert(Api<bool(*)(void*,const char*,int32_t*)>("AMediaFormat_getInt32")(output_format, "height", &height));
      assert(width > 0 && height > 0);
      Api<int(*)(void*)>("AMediaFormat_delete")(output_format);
      format_seen = true;
      ++format_changes;
    } else if (index >= 0) {
      assert(format_seen);
      BufferInfo ignored{};
      assert(dequeueOutput(codec, &ignored, 0) == -1);
      size_t capacity = 0;
      auto* buffer = Api<uint8_t*(*)(void*,size_t,size_t*)>("AMediaCodec_getOutputBuffer")(codec, index, &capacity);
      assert(buffer && capacity > 0 && capacity >= size_t(info.size));
      if (info.size > 0) {
        assert(info.pts > previous_pts);
        previous_pts = info.pts;
        ++frames;
      }
      output_eos = (info.flags & 4) != 0;
      assert(Api<int(*)(void*,size_t,bool)>("AMediaCodec_releaseOutputBuffer")(codec, index, false) == 0);
    } else assert(index == -1);
  }
  assert(output_eos && frames >= 2 && format_changes == 1);
  assert(Api<int(*)(void*)>("AMediaCodec_flush")(codec) == 0);
  assert(dequeueInput(codec, 0) == 0);
  Api<int(*)(void*)>("AMediaCodec_delete")(codec);
  Api<int(*)(void*)>("AMediaFormat_delete")(format);
  Api<int(*)(void*)>("AMediaExtractor_delete")(extractor);
  std::printf("NDK media PASS frames=%zu format-changes=%zu EOS=full-backing slot-ownership=exclusive custom-source=lifetime-safe\n", frames, format_changes);
}
