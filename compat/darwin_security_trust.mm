#include "darwin_security_trust.h"

#include <CoreFoundation/CoreFoundation.h>
#include <Security/SecTrustSettings.h>
#include <Security/Security.h>

#include <set>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cerrno>
#include <fcntl.h>
#include <string>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>

namespace {

template <typename T> class ScopedCf {
public:
  explicit ScopedCf(T value = nullptr) : value_(value) {}
  ~ScopedCf() {
    if (value_ != nullptr)
      CFRelease(value_);
  }
  ScopedCf(const ScopedCf &) = delete;
  ScopedCf &operator=(const ScopedCf &) = delete;
  ScopedCf(ScopedCf &&other) : value_(other.value_) { other.value_ = nullptr; }
  ScopedCf &operator=(ScopedCf &&other) {
    if (this == &other)
      return *this;
    if (value_ != nullptr)
      CFRelease(value_);
    value_ = other.value_;
    other.value_ = nullptr;
    return *this;
  }
  T get() const { return value_; }

private:
  T value_;
};

jobjectArray VerifyServerChain(JNIEnv *env, jclass, jobjectArray encoded_chain,
                               jstring hostname) {
  const bool debug = std::getenv("AIM_DEBUG_SECURITY") != nullptr;
  if (encoded_chain == nullptr || env->GetArrayLength(encoded_chain) == 0)
    return nullptr;
  if (debug)
    std::fprintf(stderr, "DARWIN security: macOS verify chain=%d host=%s\n",
                 env->GetArrayLength(encoded_chain),
                 hostname == nullptr ? "<null>" : "present");

  ScopedCf<CFMutableArrayRef> certificates(
      CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks));
  if (certificates.get() == nullptr)
    return nullptr;
  const jsize input_count = env->GetArrayLength(encoded_chain);
  for (jsize index = 0; index < input_count; ++index) {
    auto bytes = static_cast<jbyteArray>(
        env->GetObjectArrayElement(encoded_chain, index));
    if (bytes == nullptr)
      return nullptr;
    const jsize size = env->GetArrayLength(bytes);
    std::vector<UInt8> storage(static_cast<size_t>(size));
    env->GetByteArrayRegion(bytes, 0, size,
                            reinterpret_cast<jbyte *>(storage.data()));
    env->DeleteLocalRef(bytes);
    if (env->ExceptionCheck())
      return nullptr;
    ScopedCf<CFDataRef> data(
        CFDataCreate(kCFAllocatorDefault, storage.data(), storage.size()));
    ScopedCf<SecCertificateRef> certificate(
        data.get() == nullptr
            ? nullptr
            : SecCertificateCreateWithData(kCFAllocatorDefault, data.get()));
    if (certificate.get() == nullptr)
      return nullptr;
    CFArrayAppendValue(certificates.get(), certificate.get());
  }

  ScopedCf<CFStringRef> host;
  if (hostname != nullptr) {
    const jchar *characters = env->GetStringChars(hostname, nullptr);
    if (characters == nullptr)
      return nullptr;
    host = ScopedCf<CFStringRef>(CFStringCreateWithCharacters(
        kCFAllocatorDefault, reinterpret_cast<const UniChar *>(characters),
        env->GetStringLength(hostname)));
    env->ReleaseStringChars(hostname, characters);
  }
  ScopedCf<SecPolicyRef> policy(SecPolicyCreateSSL(true, host.get()));
  SecTrustRef raw_trust = nullptr;
  if (policy.get() == nullptr ||
      SecTrustCreateWithCertificates(certificates.get(), policy.get(),
                                     &raw_trust) != errSecSuccess) {
    return nullptr;
  }
  ScopedCf<SecTrustRef> trust(raw_trust);
  SecTrustSetNetworkFetchAllowed(trust.get(), true);
  CFErrorRef raw_error = nullptr;
  if (!SecTrustEvaluateWithError(trust.get(), &raw_error)) {
    ScopedCf<CFErrorRef> error(raw_error);
    if (debug) {
      const CFIndex code = error.get() == nullptr ? 0 : CFErrorGetCode(error.get());
      std::fprintf(stderr, "DARWIN security: macOS rejected chain code=%ld\n",
                   static_cast<long>(code));
    }
    return nullptr;
  }

  ScopedCf<CFArrayRef> trusted_chain(SecTrustCopyCertificateChain(trust.get()));
  if (trusted_chain.get() == nullptr)
    return nullptr;
  jclass byte_array_class = env->FindClass("[B");
  if (byte_array_class == nullptr)
    return nullptr;
  const CFIndex trusted_count = CFArrayGetCount(trusted_chain.get());
  jobjectArray result = env->NewObjectArray(static_cast<jsize>(trusted_count),
                                            byte_array_class, nullptr);
  env->DeleteLocalRef(byte_array_class);
  if (result == nullptr)
    return nullptr;
  for (CFIndex index = 0; index < trusted_count; ++index) {
    auto certificate = static_cast<SecCertificateRef>(
        const_cast<void *>(CFArrayGetValueAtIndex(trusted_chain.get(), index)));
    ScopedCf<CFDataRef> data(SecCertificateCopyData(certificate));
    if (data.get() == nullptr)
      return nullptr;
    const CFIndex size = CFDataGetLength(data.get());
    jbyteArray bytes = env->NewByteArray(static_cast<jsize>(size));
    if (bytes == nullptr)
      return nullptr;
    env->SetByteArrayRegion(
        bytes, 0, static_cast<jsize>(size),
        reinterpret_cast<const jbyte *>(CFDataGetBytePtr(data.get())));
    env->SetObjectArrayElement(result, static_cast<jsize>(index), bytes);
    env->DeleteLocalRef(bytes);
    if (env->ExceptionCheck())
      return nullptr;
  }
  return result;
}

void AppendCertificate(SecCertificateRef certificate,
                       std::set<std::string> *encoded) {
  ScopedCf<CFDataRef> data(SecCertificateCopyData(certificate));
  if (data.get() == nullptr)
    return;
  encoded->emplace(reinterpret_cast<const char *>(CFDataGetBytePtr(data.get())),
                   static_cast<size_t>(CFDataGetLength(data.get())));
}

bool HasRootTrustSetting(SecCertificateRef certificate,
                         SecTrustSettingsDomain domain) {
  CFArrayRef raw_settings = nullptr;
  const OSStatus status =
      SecTrustSettingsCopyTrustSettings(certificate, domain, &raw_settings);
  if (status != errSecSuccess || raw_settings == nullptr)
    return false;
  ScopedCf<CFArrayRef> settings(raw_settings);
  bool trusted = CFArrayGetCount(settings.get()) == 0;
  for (CFIndex index = 0; index < CFArrayGetCount(settings.get()); ++index) {
    auto entry = static_cast<CFDictionaryRef>(
        CFArrayGetValueAtIndex(settings.get(), index));
    if (entry == nullptr || CFGetTypeID(entry) != CFDictionaryGetTypeID())
      continue;
    auto value = static_cast<CFNumberRef>(
        CFDictionaryGetValue(entry, kSecTrustSettingsResult));
    int32_t result = kSecTrustSettingsResultTrustRoot;
    if (value != nullptr &&
        !CFNumberGetValue(value, kCFNumberSInt32Type, &result))
      continue;
    if (result == kSecTrustSettingsResultDeny)
      return false;
    if (result == kSecTrustSettingsResultTrustRoot ||
        result == kSecTrustSettingsResultTrustAsRoot)
      trusted = true;
  }
  return trusted;
}

jobjectArray CopyTrustedRoots(JNIEnv *env, jclass) {
  std::set<std::string> encoded;
  CFArrayRef raw_anchors = nullptr;
  if (SecTrustCopyAnchorCertificates(&raw_anchors) == errSecSuccess &&
      raw_anchors != nullptr) {
    ScopedCf<CFArrayRef> anchors(raw_anchors);
    for (CFIndex index = 0; index < CFArrayGetCount(anchors.get()); ++index) {
      AppendCertificate(static_cast<SecCertificateRef>(const_cast<void *>(
                            CFArrayGetValueAtIndex(anchors.get(), index))),
                        &encoded);
    }
  }
  for (SecTrustSettingsDomain domain :
       {kSecTrustSettingsDomainUser, kSecTrustSettingsDomainAdmin}) {
    CFArrayRef raw_certificates = nullptr;
    if (SecTrustSettingsCopyCertificates(domain, &raw_certificates) !=
            errSecSuccess ||
        raw_certificates == nullptr)
      continue;
    ScopedCf<CFArrayRef> certificates(raw_certificates);
    for (CFIndex index = 0; index < CFArrayGetCount(certificates.get());
         ++index) {
      auto certificate = static_cast<SecCertificateRef>(const_cast<void *>(
          CFArrayGetValueAtIndex(certificates.get(), index)));
      if (HasRootTrustSetting(certificate, domain))
        AppendCertificate(certificate, &encoded);
    }
  }
  jclass byte_array_class = env->FindClass("[B");
  if (byte_array_class == nullptr)
    return nullptr;
  jobjectArray result = env->NewObjectArray(static_cast<jsize>(encoded.size()),
                                            byte_array_class, nullptr);
  env->DeleteLocalRef(byte_array_class);
  if (result == nullptr)
    return nullptr;
  jsize index = 0;
  for (const std::string &certificate : encoded) {
    jbyteArray bytes =
        env->NewByteArray(static_cast<jsize>(certificate.size()));
    if (bytes == nullptr)
      return nullptr;
    env->SetByteArrayRegion(
        bytes, 0, static_cast<jsize>(certificate.size()),
        reinterpret_cast<const jbyte *>(certificate.data()));
    env->SetObjectArrayElement(result, index++, bytes);
    env->DeleteLocalRef(bytes);
    if (env->ExceptionCheck())
      return nullptr;
  }
  if (std::getenv("AIM_DEBUG_SECURITY") != nullptr)
    std::fprintf(stderr, "DARWIN security: exported macOS roots=%zu\n",
                 encoded.size());
  return result;
}

bool IsSystemCertificateFilename(const char *filename) {
  if (filename == nullptr || std::strlen(filename) < 10)
    return false;
  for (size_t index = 0; index < 8; ++index) {
    const char value = filename[index];
    if (!((value >= '0' && value <= '9') ||
          (value >= 'a' && value <= 'f')))
      return false;
  }
  if (filename[8] != '.')
    return false;
  for (size_t index = 9; filename[index] != '\0'; ++index) {
    if (filename[index] < '0' || filename[index] > '9')
      return false;
  }
  return filename[9] != '\0';
}

jboolean WriteSystemCertificate(JNIEnv *env, jclass, jstring filename,
                                jbyteArray encoded) {
  const char *system_root = std::getenv("AIM_ANDROID_SYSTEM_ROOT");
  if (system_root == nullptr || system_root[0] != '/' || filename == nullptr ||
      encoded == nullptr)
    return JNI_FALSE;
  const char *name = env->GetStringUTFChars(filename, nullptr);
  if (name == nullptr)
    return JNI_FALSE;
  if (!IsSystemCertificateFilename(name)) {
    env->ReleaseStringUTFChars(filename, name);
    return JNI_FALSE;
  }
  const std::string system(system_root);
  const std::string etc = system + "/etc";
  const std::string security = etc + "/security";
  const std::string cacerts = security + "/cacerts";
  chmod(system.c_str(), 0700);
  chmod(etc.c_str(), 0700);
  if ((mkdir(security.c_str(), 0700) != 0 && errno != EEXIST) ||
      (mkdir(cacerts.c_str(), 0700) != 0 && errno != EEXIST)) {
    chmod(etc.c_str(), 0500);
    chmod(system.c_str(), 0500);
    env->ReleaseStringUTFChars(filename, name);
    return JNI_FALSE;
  }
  chmod(security.c_str(), 0700);
  chmod(cacerts.c_str(), 0700);
  const std::string path = cacerts + "/" + name;
  env->ReleaseStringUTFChars(filename, name);
  const jsize size = env->GetArrayLength(encoded);
  std::vector<jbyte> bytes(static_cast<size_t>(size));
  env->GetByteArrayRegion(encoded, 0, size, bytes.data());
  if (env->ExceptionCheck())
    return JNI_FALSE;
  int fd;
  do {
    fd = open(path.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0400);
  } while (fd < 0 && errno == EINTR);
  bool written = fd >= 0;
  size_t offset = 0;
  while (written && offset < bytes.size()) {
    const ssize_t count = write(fd, bytes.data() + offset, bytes.size() - offset);
    if (count > 0) {
      offset += static_cast<size_t>(count);
    } else if (count < 0 && errno == EINTR) {
      continue;
    } else {
      written = false;
    }
  }
  if (fd >= 0) {
    fchmod(fd, 0400);
    close(fd);
  }
  chmod(cacerts.c_str(), 0500);
  chmod(security.c_str(), 0500);
  chmod(etc.c_str(), 0500);
  chmod(system.c_str(), 0500);
  return written ? JNI_TRUE : JNI_FALSE;
}

} // namespace

namespace aim {

bool RegisterDarwinSecurityTrustNatives(JNIEnv *env) {
  jclass manager = env->FindClass(
      "dev/aim/security/DarwinTrustManagerFactory$DarwinTrustManager");
  if (manager == nullptr)
    return false;
  JNINativeMethod methods[] = {{
      const_cast<char *>("verifyServerChain"),
      const_cast<char *>("([[BLjava/lang/String;)[[B"),
      reinterpret_cast<void *>(&VerifyServerChain),
  }};
  const bool registered = env->RegisterNatives(manager, methods, 1) == JNI_OK;
  env->DeleteLocalRef(manager);
  if (!registered)
    return false;
  jclass store = env->FindClass("dev/aim/security/DarwinAndroidCAStore");
  if (store == nullptr)
    return false;
  JNINativeMethod store_methods[] = {
      {
          const_cast<char *>("copyTrustedRoots"),
          const_cast<char *>("()[[B"),
          reinterpret_cast<void *>(&CopyTrustedRoots),
      },
      {
          const_cast<char *>("writeSystemCertificate"),
          const_cast<char *>("(Ljava/lang/String;[B)Z"),
          reinterpret_cast<void *>(&WriteSystemCertificate),
      },
  };
  const bool store_registered =
      env->RegisterNatives(store, store_methods, 2) == JNI_OK;
  env->DeleteLocalRef(store);
  return store_registered;
}

} // namespace aim
