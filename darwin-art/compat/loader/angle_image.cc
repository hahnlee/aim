#include "angle_image.h"

#include "darwin_angle_egl.h"
#include "gles1_exports.h"
#include <algorithm>

#include <cstring>
#include <mutex>
#include <new>
#include <string>
#include <unordered_set>

namespace darwin_art::loader {
namespace {

constexpr char kEglSoname[] = "libEGL.so";
constexpr char kGlesSoname[] = "libGLESv2.so";
constexpr char kGles3Soname[] = "libGLESv3.so";
constexpr char kGles1Soname[] = "libGLESv1_CM.so";
constexpr uint64_t kAngleImageTag = 0x44415257494e4147ULL;  // "DARWINAG".

struct AngleImagePayload {
  uint64_t tag;
  std::string soname;
  std::string canonical;
};

// The registry's ANGLE representation is private to this owner. Keep an
// owner-local identity set so checking a lease never dereferences a payload
// belonging to another image owner. The bookkeeping intentionally lives for
// process lifetime; payloads themselves are removed on release.
std::mutex& PayloadMutex() {
  static auto* mutex = new std::mutex();
  return *mutex;
}

std::unordered_set<const AngleImagePayload*>& LivePayloads() {
  static auto* payloads = new std::unordered_set<const AngleImagePayload*>();
  return *payloads;
}

bool IsSupportedSoname(const char* soname) {
  return soname && (std::strcmp(soname, kEglSoname) == 0 ||
                    std::strcmp(soname, kGlesSoname) == 0 ||
                    std::strcmp(soname, kGles3Soname) == 0 ||
                    std::strcmp(soname, kGles1Soname) == 0);
}

// Android's separate GLESv2/v3 dispatch DSOs expose the same API inventory.
// ANGLE provides both context versions from one Darwin backend DSO; preserve
// separate Android image identities without executing Bionic TLS dispatch code.
const char* BackendSoname(const char* soname) {
  return std::strcmp(soname, kEglSoname) == 0 ? kEglSoname : kGlesSoname;
}

const char* ReadinessSymbol(const char* soname) {
  if (std::strcmp(soname, kEglSoname) == 0) return "eglGetError";
  if (std::strcmp(soname, kGles1Soname) == 0) return "glMatrixMode";
  if (std::strcmp(soname, kGlesSoname) == 0 || std::strcmp(soname, kGles3Soname) == 0) return "glGetError";
  return nullptr;
}

const char* SymbolPrefix(const char* soname) {
  if (std::strcmp(soname, kEglSoname) == 0) return "egl";
  if (std::strcmp(soname, kGles1Soname) == 0) return "gl";
  if (std::strcmp(soname, kGlesSoname) == 0 || std::strcmp(soname, kGles3Soname) == 0) return "gl";
  return nullptr;
}

bool IsLivePayload(const void* payload, std::string* soname) {
  if (!payload) return false;
  const auto* image = static_cast<const AngleImagePayload*>(payload);
  std::lock_guard<std::mutex> lock(PayloadMutex());
  const auto it = LivePayloads().find(image);
  if (it == LivePayloads().end() || image->tag != kAngleImageTag ||
      !IsSupportedSoname(image->soname.c_str())) {
    return false;
  }
  if (soname) *soname = image->soname;
  return true;
}

bool LeaseSoname(const LinkerImageLease* lease, std::string* soname) {
  if (soname) soname->clear();
  if (!lease) return false;
  void* payload = nullptr;
  if (darwin_art_linker_image_typed_payload(
      lease, DARWIN_ART_IMAGE_ANGLE, &payload) != 0) {
    return false;
  }
  return IsLivePayload(payload, soname);
}

void* Retain(void* source) {
  if (!source) return nullptr;
  const auto* image = static_cast<const AngleImagePayload*>(source);
  // The callback is installed only with our stack payload. This check also
  // prevents accidental publication of a payload with a different owner.
  if (image->tag != kAngleImageTag ||
      !IsSupportedSoname(image->soname.c_str())) {
    return nullptr;
  }
  AngleImagePayload* retained = nullptr;
  try {
    retained = new AngleImagePayload(*image);
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
  auto* image = static_cast<AngleImagePayload*>(source);
  {
    std::lock_guard<std::mutex> lock(PayloadMutex());
    LivePayloads().erase(image);
  }
  delete image;
}

void SetError(std::string* error, const char* message) {
  if (error) *error = message;
}

bool IsPublicDispatchSymbol(const char* symbol, const char* prefix) {
  if (!symbol || !prefix) return false;
  const size_t prefix_length = std::strlen(prefix);
  if (std::strncmp(symbol, prefix, prefix_length) != 0) return false;
  // Public EGL/GLES entry points use the prefix plus UpperCamelCase form.
  // This rejects private/underscore and cross-dispatch symbols.
  return symbol[prefix_length] >= 'A' && symbol[prefix_length] <= 'Z';
}

}  // namespace

bool PublishAngleImage(LinkerRegistry* registry, uint64_t id,
    const char* soname, const char* canonical, std::string* error) {
  if (error) error->clear();
  if (!registry || !IsSupportedSoname(soname) || !canonical ||
      canonical[0] != '/') {
    SetError(error, "ANGLE image requires a supported SONAME and absolute canonical path");
    return false;
  }

  // GetAngleApi() owns the backend handles for the process lifetime. This
  // image retains only immutable identity and never closes an ANGLE handle.
  const char* readiness = ReadinessSymbol(soname);
  if (!readiness || !::darwin_art::darwin_art_angle_dso_symbol(BackendSoname(soname), readiness)) {
    SetError(error, "ANGLE dispatch image is not ready");
    return false;
  }

  try {
    AngleImagePayload image{kAngleImageTag, soname, canonical};
    if (darwin_art_linker_namespace_publish_image(registry, id, soname,
        canonical, &image, Retain, Release, 0, 0, DARWIN_ART_IMAGE_ANGLE) != 0) {
      SetError(error, "ANGLE image publication failed");
      return false;
    }
  } catch (...) {
    SetError(error, "ANGLE image allocation failed");
    return false;
  }
  return true;
}

bool IsAngleImage(const LinkerImageLease* lease) {
  return LeaseSoname(lease, nullptr);
}

int ResolveAngleImage(const LinkerImageLease* lease, const char* symbol,
    const char* version, uintptr_t* out, std::string* error) {
  if (error) error->clear();
  if (out) *out = 0;
  if (!lease || !symbol || !*symbol || !out) {
    SetError(error, "invalid ANGLE image lookup arguments");
    return -1;
  }

  std::string soname;
  if (!LeaseSoname(lease, &soname)) {
    SetError(error, "lease is not an ANGLE image owned by this provider");
    return -1;
  }
  const bool gles1 = soname == kGles1Soname;
  if (version && !(gles1 && std::strcmp(version, "LIBGLESV1_CM") == 0)) {
    SetError(error, "versioned ANGLE symbols are unsupported");
    return 1;
  }
  if (!IsPublicDispatchSymbol(symbol, SymbolPrefix(soname.c_str()))) {
    SetError(error, "private or cross-dispatch symbol rejected");
    return 1;
  }
  if (gles1 && !std::binary_search(std::begin(kGles1Exports), std::end(kGles1Exports),
      std::string_view(symbol))) {
    SetError(error, "symbol is not exported by Android GLES1");
    return 1;
  }

  void* address = ::darwin_art::darwin_art_angle_dso_symbol(
      BackendSoname(soname.c_str()), symbol);
  if (!address) {
    SetError(error, "ANGLE symbol is unavailable");
    return 1;
  }
  *out = reinterpret_cast<uintptr_t>(address);
  return 0;
}

}  // namespace darwin_art::loader
