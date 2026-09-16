#include "native_window_image.h"

// Opaque Android ABI pointer; implementations are required link dependencies.
struct ANativeWindow;

#include <cstring>
#include <mutex>
#include <new>
#include <string>
#include <unordered_set>

extern "C" {
void* darwin_art_android_platform_symbol(const char* symbol);
int32_t darwin_art_android_ANativeWindow_lock(void* window, void* buffer,
                                               void* dirty_bounds);
int32_t darwin_art_android_ANativeWindow_unlockAndPost(void* window);
int32_t darwin_art_android_ANativeWindow_setBuffersGeometry(
    void* window, int32_t width, int32_t height, int32_t format);
int32_t ANativeWindow_setBuffersDataSpace(ANativeWindow* window,
                                           int32_t dataspace);
int32_t ANativeWindow_getBuffersDataSpace(ANativeWindow* window);
int32_t ANativeWindow_setFrameRate(ANativeWindow* window, float frame_rate,
                                   int8_t compatibility);
int32_t ANativeWindow_setFrameRateWithChangeStrategy(
    ANativeWindow* window, float frame_rate, int8_t compatibility,
    int8_t strategy);
}

namespace darwin_art::loader {
namespace {

constexpr char kSoname[] = "libnativewindow.so";
constexpr char kVersion[] = "LIBNATIVEWINDOW";
constexpr uint64_t kPayloadTag = 0x44415257494e4e57ULL;  // "DARWINNW".

struct NativeWindowImagePayload {
  uint64_t tag;
  std::string soname;
  std::string canonical;
};

// A lease's payload is borrowed.  Validate membership before reading it so
// an arbitrary image owned by another provider can never be cast here.
std::mutex& PayloadMutex() {
  static auto* mutex = new std::mutex();
  return *mutex;
}

std::unordered_set<const NativeWindowImagePayload*>& LivePayloads() {
  static auto* payloads = new std::unordered_set<const NativeWindowImagePayload*>();
  return *payloads;
}

void SetError(std::string* error, const char* message) {
  if (error) *error = message;
}

bool IsOurSoname(const char* soname) {
  return soname != nullptr && std::strcmp(soname, kSoname) == 0;
}

bool IsLivePayload(const void* payload) {
  if (!payload) return false;
  const auto* image = static_cast<const NativeWindowImagePayload*>(payload);
  std::lock_guard<std::mutex> lock(PayloadMutex());
  const auto it = LivePayloads().find(image);
  return it != LivePayloads().end() && image->tag == kPayloadTag &&
      IsOurSoname(image->soname.c_str());
}

bool LeasePayload(const LinkerImageLease* lease,
                  const NativeWindowImagePayload** out) {
  if (out) *out = nullptr;
  if (!lease) return false;
  void* payload = nullptr;
  if (darwin_art_linker_image_typed_payload(
          lease, DARWIN_ART_IMAGE_NATIVE_WINDOW, &payload) != 0 ||
      !IsLivePayload(payload)) {
    return false;
  }
  if (out) *out = static_cast<const NativeWindowImagePayload*>(payload);
  return true;
}

void* Retain(void* source) {
  if (!source) return nullptr;
  const auto* image = static_cast<const NativeWindowImagePayload*>(source);
  if (image->tag != kPayloadTag || !IsOurSoname(image->soname.c_str()))
    return nullptr;

  NativeWindowImagePayload* retained = nullptr;
  try {
    retained = new NativeWindowImagePayload(*image);
    std::lock_guard<std::mutex> lock(PayloadMutex());
    LivePayloads().insert(retained);
  } catch (...) {
    delete retained;
    return nullptr;
  }
  return retained;
}

void Release(void* source) {
  if (!source) return;
  auto* image = static_cast<NativeWindowImagePayload*>(source);
  {
    std::lock_guard<std::mutex> lock(PayloadMutex());
    LivePayloads().erase(image);
  }
  delete image;
}

// Strong function references below make a missing backend a link error.
bool VersionMatches(const char* version) {
  return version == nullptr || std::strcmp(version, kVersion) == 0;
}

uintptr_t LookupSymbol(const char* symbol) {
  if (std::strcmp(symbol, "ANativeWindow_acquire") == 0)
    return reinterpret_cast<uintptr_t>(darwin_art_android_platform_symbol(symbol));
  if (std::strcmp(symbol, "ANativeWindow_getFormat") == 0)
    return reinterpret_cast<uintptr_t>(darwin_art_android_platform_symbol(symbol));
  if (std::strcmp(symbol, "ANativeWindow_getHeight") == 0)
    return reinterpret_cast<uintptr_t>(darwin_art_android_platform_symbol(symbol));
  if (std::strcmp(symbol, "ANativeWindow_getWidth") == 0)
    return reinterpret_cast<uintptr_t>(darwin_art_android_platform_symbol(symbol));
  if (std::strcmp(symbol, "ANativeWindow_lock") == 0)
    return reinterpret_cast<uintptr_t>(&darwin_art_android_ANativeWindow_lock);
  if (std::strcmp(symbol, "ANativeWindow_release") == 0)
    return reinterpret_cast<uintptr_t>(darwin_art_android_platform_symbol(symbol));
  if (std::strcmp(symbol, "ANativeWindow_setBuffersDataSpace") == 0)
    return reinterpret_cast<uintptr_t>(&ANativeWindow_setBuffersDataSpace);
  if (std::strcmp(symbol, "ANativeWindow_getBuffersDataSpace") == 0)
    return reinterpret_cast<uintptr_t>(&ANativeWindow_getBuffersDataSpace);
  if (std::strcmp(symbol, "ANativeWindow_setBuffersGeometry") == 0)
    return reinterpret_cast<uintptr_t>(&darwin_art_android_ANativeWindow_setBuffersGeometry);
  if (std::strcmp(symbol, "ANativeWindow_setFrameRate") == 0)
    return reinterpret_cast<uintptr_t>(&ANativeWindow_setFrameRate);
  if (std::strcmp(symbol, "ANativeWindow_setFrameRateWithChangeStrategy") == 0)
    return reinterpret_cast<uintptr_t>(&ANativeWindow_setFrameRateWithChangeStrategy);
  if (std::strcmp(symbol, "ANativeWindow_unlockAndPost") == 0)
    return reinterpret_cast<uintptr_t>(&darwin_art_android_ANativeWindow_unlockAndPost);
  return 0;
}

}  // namespace

bool PublishNativeWindowImage(LinkerRegistry* registry, uint64_t id,
                              const char* soname, const char* canonical,
                              std::string* error) {
  if (error) error->clear();
  if (!registry || !IsOurSoname(soname) || !canonical || canonical[0] != '/') {
    SetError(error, "native-window image requires libnativewindow.so and an absolute canonical path");
    return false;
  }

  try {
    NativeWindowImagePayload image{kPayloadTag, soname, canonical};
    if (darwin_art_linker_namespace_publish_image(
            registry, id, soname, canonical, &image, Retain, Release, 0, 0,
            DARWIN_ART_IMAGE_NATIVE_WINDOW) != 0) {
      SetError(error, "native-window image publication failed");
      return false;
    }
  } catch (...) {
    SetError(error, "native-window image allocation failed");
    return false;
  }
  return true;
}

bool IsNativeWindowImage(const LinkerImageLease* lease) {
  return LeasePayload(lease, nullptr);
}

int ResolveNativeWindowImage(const LinkerImageLease* lease, const char* symbol,
                             const char* version, uintptr_t* out,
                             std::string* error) {
  if (error) error->clear();
  if (out) *out = 0;
  if (!lease || !symbol || !*symbol || !out) {
    SetError(error, "invalid native-window image lookup arguments");
    return -1;
  }
  const NativeWindowImagePayload* image = nullptr;
  if (!LeasePayload(lease, &image)) {
    SetError(error, "lease is not a native-window image owned by this provider");
    return -1;
  }
  (void)image;
  if (!VersionMatches(version)) {
    SetError(error, "native-window symbol version is unsupported");
    return 1;
  }
  const uintptr_t address = LookupSymbol(symbol);
  if (!address) {
    SetError(error, "native-window symbol is unavailable");
    return 1;
  }
  *out = address;
  return 0;
}

}  // namespace darwin_art::loader
