#include "overlay_config_jni.h"

#include "../filesystem/archive_open.h"

#include <algorithm>
#include <array>
#include <cstdint>
#include <cstring>
#include <fcntl.h>
#include <iostream>
#include <optional>
#include <string>
#include <string_view>
#include <unistd.h>
#include <utility>
#include <vector>

// AOSP's createIdmap runs `/system/bin/idmap2 create-multiple`, which
// (re)writes /data/resource-cache/<overlay path>@idmap for each overlay and
// prints the path of every idmap that is up to date afterwards; an overlay
// it cannot map is left out. This image is immutable, so image assembly ran
// the same idmap2 command once (tools/lib/package-system-root.sh) and ships
// the idmaps in /system/etc/resource-cache. Here each shipped idmap is
// checked the way idmap2's Verify checks an existing one - header magic and
// version, fulfilled policies, overlayable enforcement, target and overlay
// paths - and returned only when it matches the request (ADR 0009).
namespace darwin_art::content {
namespace {

constexpr std::string_view kResourceCache = "/system/etc/resource-cache";
// androidfw/ResourceTypes.h kIdmapMagic / kIdmapCurrentVersion.
constexpr uint32_t kIdmapMagic = 0x504D4449u;
constexpr uint32_t kIdmapCurrentVersion = 0x0000000Bu;
// ResTable_overlayable_policy_header::PolicyFlags, keyed by idmap2's names.
constexpr std::array<std::pair<std::string_view, uint32_t>, 9> kPolicies = {{
    {"public", 0x001}, {"system", 0x002}, {"vendor", 0x004},
    {"product", 0x008}, {"signature", 0x010}, {"odm", 0x020},
    {"oem", 0x040}, {"actor", 0x080}, {"config_signature", 0x100},
}};

std::string Utf8(JNIEnv* env, jstring value) {
  if (value == nullptr) return {};
  const char* text = env->GetStringUTFChars(value, nullptr);
  if (text == nullptr) return {};
  std::string result(text);
  env->ReleaseStringUTFChars(value, text);
  return result;
}

// Idmap::CanonicalIdmapPathFor.
std::string IdmapPathFor(std::string_view overlay_path) {
  std::string mangled(overlay_path.substr(1));
  std::replace(mangled.begin(), mangled.end(), '/', '@');
  return std::string(kResourceCache) + "/" + mangled + "@idmap";
}

std::optional<std::string> ReadGuestFile(const std::string& path) {
  const int fd = darwin_art_archive_open(path.c_str(), O_RDONLY | O_CLOEXEC);
  if (fd < 0) return std::nullopt;
  std::string contents;
  char buffer[4096];
  ssize_t count = 0;
  while ((count = read(fd, buffer, sizeof(buffer))) > 0) contents.append(buffer, count);
  close(fd);
  if (count < 0) return std::nullopt;
  return contents;
}

class HeaderReader {
 public:
  explicit HeaderReader(std::string_view data) : data_(data) {}
  std::optional<uint32_t> Word() {
    if (data_.size() - offset_ < 4) return std::nullopt;
    uint32_t value = 0;
    std::memcpy(&value, data_.data() + offset_, 4);  // little-endian host
    offset_ += 4;
    return value;
  }
  // BinaryStreamVisitor::WriteString: length, bytes, NUL padding to a word.
  std::optional<std::string_view> String() {
    const auto length = Word();
    if (!length || data_.size() - offset_ < *length) return std::nullopt;
    const std::string_view value = data_.substr(offset_, *length);
    offset_ += (*length + 3u) & ~3u;
    if (offset_ > data_.size()) return std::nullopt;
    return value;
  }

 private:
  std::string_view data_;
  size_t offset_ = 0;
};

// Why the idmap does not match, or empty when it does.
std::string Mismatch(std::string_view idmap, std::string_view target,
                     std::string_view overlay, uint32_t policies, bool enforce) {
  HeaderReader header(idmap);
  const auto magic = header.Word();
  const auto version = header.Word();
  if (magic != kIdmapMagic || version != kIdmapCurrentVersion) return "not a current idmap";
  if (!header.Word() || !header.Word()) return "truncated header";  // target/overlay CRCs
  if (header.Word() != policies) return "fulfilled policies differ";
  if (header.Word() != (enforce ? 1u : 0u)) return "overlayable enforcement differs";
  if (header.String() != target) return "target path differs";
  if (header.String() != overlay) return "overlay path differs";
  return {};
}

jobjectArray CreateIdmap(JNIEnv* env, jclass, jstring target_path,
                         jobjectArray overlay_paths, jobjectArray policy_names,
                         jboolean enforce_overlayable) {
  const std::string target = Utf8(env, target_path);
  uint32_t policies = 0;
  for (jsize i = 0, n = env->GetArrayLength(policy_names); i < n; ++i) {
    auto element = static_cast<jstring>(env->GetObjectArrayElement(policy_names, i));
    const std::string name = Utf8(env, element);
    env->DeleteLocalRef(element);
    const auto policy = std::find_if(kPolicies.begin(), kPolicies.end(),
                                     [&](const auto& entry) { return entry.first == name; });
    if (policy == kPolicies.end()) {
      // idmap2 rejects the invocation; the caller loads no overlays.
      std::cerr << "OverlayConfig: idmap2: invalid policy '" << name << "'\n";
      return nullptr;
    }
    policies |= policy->second;
  }
  if (policies == 0) policies = 0x001;

  std::vector<std::string> idmaps;
  for (jsize i = 0, n = env->GetArrayLength(overlay_paths); i < n; ++i) {
    auto element = static_cast<jstring>(env->GetObjectArrayElement(overlay_paths, i));
    const std::string overlay = Utf8(env, element);
    env->DeleteLocalRef(element);
    if (overlay.empty() || overlay[0] != '/') continue;
    const std::string idmap_path = IdmapPathFor(overlay);
    const auto idmap = ReadGuestFile(idmap_path);
    const std::string mismatch =
        idmap ? Mismatch(*idmap, target, overlay, policies, enforce_overlayable == JNI_TRUE)
              : "no idmap was created at image assembly";
    if (!mismatch.empty()) {
      std::cerr << "OverlayConfig: idmap for " << overlay << " unavailable: " << mismatch << "\n";
      continue;
    }
    idmaps.push_back(idmap_path);
  }

  jclass string_class = env->FindClass("java/lang/String");
  if (string_class == nullptr) return nullptr;
  jobjectArray result = env->NewObjectArray(static_cast<jsize>(idmaps.size()), string_class, nullptr);
  env->DeleteLocalRef(string_class);
  if (result == nullptr) return nullptr;
  for (size_t i = 0; i < idmaps.size(); ++i) {
    jstring path = env->NewStringUTF(idmaps[i].c_str());
    if (path == nullptr) return nullptr;
    env->SetObjectArrayElement(result, static_cast<jsize>(i), path);
    env->DeleteLocalRef(path);
  }
  return result;
}

}  // namespace

bool RegisterOverlayConfigNatives(JNIEnv* env) {
  jclass overlay_config = env->FindClass("com/android/internal/content/om/OverlayConfig");
  if (overlay_config == nullptr) return false;
  const JNINativeMethod methods[] = {
      {const_cast<char*>("createIdmap"),
       const_cast<char*>("(Ljava/lang/String;[Ljava/lang/String;[Ljava/lang/String;Z)[Ljava/lang/String;"),
       reinterpret_cast<void*>(CreateIdmap)},
  };
  const bool registered = env->RegisterNatives(overlay_config, methods, 1) == JNI_OK;
  env->DeleteLocalRef(overlay_config);
  return registered;
}

}  // namespace darwin_art::content
