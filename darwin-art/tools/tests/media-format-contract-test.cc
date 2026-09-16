#include "../../compat/media/format.h"
#include "../../compat/darwin_android_media_ndk.h"

#include <media/NdkMediaFormat.h>

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>

namespace {

template <typename Function> Function Api(const char *name) {
#if defined(DARWIN_ART_TEST_MEDIA_COMPOSITE)
  void *symbol = darwin_art_android_media_ndk_symbol(name);
#else
  void *symbol = darwin_art::media::FormatSymbol(name);
#endif
  assert(symbol != nullptr);
  return reinterpret_cast<Function>(symbol);
}

void TestSizeTypeAndNullDefense() {
  auto make = Api<AMediaFormat *(*)()>("AMediaFormat_new");
  auto set_size = Api<void (*)(AMediaFormat *, const char *, size_t)>(
      "AMediaFormat_setSize");
  auto get_size = Api<bool (*)(AMediaFormat *, const char *, size_t *)>(
      "AMediaFormat_getSize");
  auto get_int64 = Api<bool (*)(AMediaFormat *, const char *, int64_t *)>(
      "AMediaFormat_getInt64");
  auto destroy = Api<media_status_t (*)(AMediaFormat *)>("AMediaFormat_delete");

  AMediaFormat *format = make();
  assert(format != nullptr);
  set_size(format, "buffer-size", 1234);
  size_t size = 0;
  assert(get_size(format, "buffer-size", &size));
  assert(size == 1234);
  int64_t wide = 0;
  assert(!get_int64(format, "buffer-size", &wide));
  // AOSP's AMediaFormat_getSize implementation dereferences its output
  // pointer.  This null-output rejection is an intentional defensive check
  // in the facade, not an AOSP nullability guarantee.
  assert(!get_size(format, "buffer-size", nullptr));
  assert(destroy(format) == AMEDIA_OK);
}

void TestTypeReplacementAndMismatch() {
  auto make = Api<AMediaFormat *(*)()>("AMediaFormat_new");
  auto destroy = Api<media_status_t (*)(AMediaFormat *)>("AMediaFormat_delete");
  auto set_int32 = Api<void (*)(AMediaFormat *, const char *, int32_t)>(
      "AMediaFormat_setInt32");
  auto set_string = Api<void (*)(AMediaFormat *, const char *, const char *)>(
      "AMediaFormat_setString");
  auto get_int32 = Api<bool (*)(AMediaFormat *, const char *, int32_t *)>(
      "AMediaFormat_getInt32");
  auto get_int64 = Api<bool (*)(AMediaFormat *, const char *, int64_t *)>(
      "AMediaFormat_getInt64");
  auto get_string = Api<bool (*)(AMediaFormat *, const char *, const char **)>(
      "AMediaFormat_getString");

  AMediaFormat *format = make();
  assert(format != nullptr);
  set_int32(format, "codec-param", 7);
  int32_t narrow = 0;
  int64_t wide = 0;
  assert(get_int32(format, "codec-param", &narrow) && narrow == 7);
  assert(!get_int64(format, "codec-param", &wide));

  // A key has one current value in AOSP's AMediaFormat. Replacing its type
  // must invalidate the old typed getter, not leave parallel type maps.
  set_string(format, "codec-param", "seven");
  const char *text = nullptr;
  assert(!get_int32(format, "codec-param", &narrow));
  assert(get_string(format, "codec-param", &text));
  assert(text != nullptr && std::strcmp(text, "seven") == 0);
  assert(destroy(format) == AMEDIA_OK);
}

void TestCopiedBufferAndStringInputs() {
  auto make = Api<AMediaFormat *(*)()>("AMediaFormat_new");
  auto destroy = Api<media_status_t (*)(AMediaFormat *)>("AMediaFormat_delete");
  auto set_buffer =
      Api<void (*)(AMediaFormat *, const char *, const void *, size_t)>(
          "AMediaFormat_setBuffer");
  auto get_buffer =
      Api<bool (*)(AMediaFormat *, const char *, void **, size_t *)>(
          "AMediaFormat_getBuffer");
  auto set_string = Api<void (*)(AMediaFormat *, const char *, const char *)>(
      "AMediaFormat_setString");
  auto get_string = Api<bool (*)(AMediaFormat *, const char *, const char **)>(
      "AMediaFormat_getString");

  AMediaFormat *format = make();
  assert(format != nullptr);
  uint8_t source[] = {1, 2, 3, 4};
  set_buffer(format, "csd-0", source, sizeof(source));
  source[0] = 99;
  void *data = nullptr;
  size_t size = 0;
  assert(get_buffer(format, "csd-0", &data, &size));
  assert(data != nullptr && size == sizeof(source));
  assert(std::memcmp(data, "\x01\x02\x03\x04", sizeof(source)) == 0);

  char source_string[] = "video/avc";
  set_string(format, "mime", source_string);
  source_string[0] = 'X';
  const char *copied = nullptr;
  assert(get_string(format, "mime", &copied));
  assert(copied != nullptr && std::strcmp(copied, "video/avc") == 0);

  // getString's result remains owned by the format until the next getString
  // call.  Updating another key must not invalidate the retained result.
  const char *retained = copied;
  set_string(format, "language", "eng");
  assert(std::strcmp(retained, "video/avc") == 0);
  const char *next = nullptr;
  assert(get_string(format, "language", &next));
  assert(next != nullptr && std::strcmp(next, "eng") == 0);
  assert(destroy(format) == AMEDIA_OK);
}

void TestToStringContent() {
  auto make = Api<AMediaFormat *(*)()>("AMediaFormat_new");
  auto destroy = Api<media_status_t (*)(AMediaFormat *)>("AMediaFormat_delete");
  auto set_string = Api<void (*)(AMediaFormat *, const char *, const char *)>(
      "AMediaFormat_setString");
  auto to_string =
      Api<const char *(*)(AMediaFormat *)>("AMediaFormat_toString");

  AMediaFormat *format = make();
  AMediaFormat *other = make();
  assert(format != nullptr && other != nullptr);
  set_string(format, "mime", "video/x-vnd.on2.vp9");
  const char *description = to_string(format);
  assert(description != nullptr);
  // Only the key/value content is part of this contract; punctuation and
  // type labels in the debug representation are deliberately unspecified.
  assert(std::strstr(description, "mime") != nullptr);
  assert(std::strstr(description, "video/x-vnd.on2.vp9") != nullptr);

  // Each format owns its toString result. Calling toString on another
  // object must not overwrite the first object's still-live result.
  set_string(other, "mime", "video/avc");
  const char *other_description = to_string(other);
  assert(other_description != nullptr);
  assert(std::strstr(other_description, "mime") != nullptr);
  assert(std::strstr(other_description, "video/avc") != nullptr);
  assert(std::strstr(description, "mime") != nullptr);
  assert(std::strstr(description, "video/x-vnd.on2.vp9") != nullptr);

  assert(destroy(other) == AMEDIA_OK);
  assert(destroy(format) == AMEDIA_OK);
}

} // namespace

int main() {
  TestSizeTypeAndNullDefense();
  TestTypeReplacementAndMismatch();
  TestCopiedBufferAndStringInputs();
  TestToStringContent();
  std::puts(
      "media-format-contract: PASS resolver size-null-defense "
      "typed-replacement buffer-string-copy tostring-lifetime");
  return 0;
}
