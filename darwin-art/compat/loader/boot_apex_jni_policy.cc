#include "boot_apex_jni_policy.h"

#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <array>
#include <cerrno>
#include <cstring>
#include <sstream>
#include <string>

namespace darwin_art::loader {
namespace {

constexpr size_t kMaximumPolicyBytes = 1024 * 1024;
constexpr const char* kApexPrefix = "/apex/";

bool IsAbsoluteWithoutTraversal(const char* value) {
  if (value == nullptr || value[0] != '/') return false;
  const std::string path(value);
  return path.find("/../") == std::string::npos && !path.ends_with("/..") &&
         path.find("/./") == std::string::npos && !path.ends_with("/.");
}

bool IsSafeSoname(const char* value) {
  if (value == nullptr || value[0] == '\0') return false;
  const std::string soname(value);
  return soname.find('/') == std::string::npos &&
         soname.find('\\') == std::string::npos &&
         soname.find("..") == std::string::npos && soname.ends_with(".so");
}

std::string ApexModule(const char* caller_location) {
  if (caller_location == nullptr) return {};
  const std::string location(caller_location);
  if (!location.starts_with(kApexPrefix)) return {};
  const size_t begin = std::strlen(kApexPrefix);
  const size_t slash = location.find('/', begin);
  if (slash == std::string::npos || slash == begin) return {};
  const std::string module = location.substr(begin, slash - begin);
  for (const char byte : module) {
    const bool valid = (byte >= 'a' && byte <= 'z') ||
                       (byte >= 'A' && byte <= 'Z') ||
                       (byte >= '0' && byte <= '9') || byte == '.' ||
                       byte == '_';
    if (!valid) return {};
  }
  return module;
}

std::string ApexNamespace(std::string module) {
  for (char& byte : module) {
    if (byte == '.') byte = '_';
  }
  return module;
}

bool ReadPolicy(const std::string& path, std::string* contents,
                std::string* error) {
  const int descriptor = open(path.c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  if (descriptor < 0) {
    *error = "cannot open Android APEX JNI policy: " +
             std::string(std::strerror(errno));
    return false;
  }
  struct stat status {};
  if (fstat(descriptor, &status) != 0 || !S_ISREG(status.st_mode) ||
      status.st_size < 0 ||
      static_cast<uint64_t>(status.st_size) > kMaximumPolicyBytes) {
    close(descriptor);
    *error = "Android APEX JNI policy is not a bounded regular file";
    return false;
  }
  contents->clear();
  contents->reserve(static_cast<size_t>(status.st_size));
  std::array<char, 4096> buffer{};
  while (true) {
    const ssize_t count = read(descriptor, buffer.data(), buffer.size());
    if (count == 0) break;
    if (count < 0) {
      if (errno == EINTR) continue;
      close(descriptor);
      *error = "cannot read Android APEX JNI policy";
      return false;
    }
    contents->append(buffer.data(), static_cast<size_t>(count));
    if (contents->size() > kMaximumPolicyBytes) {
      close(descriptor);
      *error = "Android APEX JNI policy exceeds size limit";
      return false;
    }
  }
  close(descriptor);
  return true;
}

bool ContainsJniLibrary(const std::string& policy,
                        const std::string& apex_namespace,
                        const std::string& soname) {
  std::istringstream lines(policy);
  std::string line;
  while (std::getline(lines, line)) {
    const size_t comment = line.find('#');
    if (comment != std::string::npos) line.resize(comment);
    std::istringstream fields(line);
    std::string tag;
    std::string namespace_name;
    std::string libraries;
    std::string extra;
    if (!(fields >> tag)) continue;
    if (!(fields >> namespace_name >> libraries) || (fields >> extra)) continue;
    if (tag != "jni" || namespace_name != apex_namespace) continue;
    size_t begin = 0;
    while (begin <= libraries.size()) {
      const size_t end = libraries.find(':', begin);
      if (libraries.substr(begin, end - begin) == soname) return true;
      if (end == std::string::npos) break;
      begin = end + 1;
    }
  }
  return false;
}

std::string PublicApexNamespace(const std::string& policy,
                                const std::string& soname) {
  std::istringstream lines(policy);
  std::string line;
  while (std::getline(lines, line)) {
    const size_t comment = line.find('#');
    if (comment != std::string::npos) line.resize(comment);
    std::istringstream fields(line);
    std::string tag;
    std::string namespace_name;
    std::string libraries;
    std::string extra;
    if (!(fields >> tag >> namespace_name >> libraries) || (fields >> extra) ||
        tag != "public") {
      continue;
    }
    size_t begin = 0;
    while (begin <= libraries.size()) {
      const size_t end = libraries.find(':', begin);
      if (libraries.substr(begin, end - begin) == soname) return namespace_name;
      if (end == std::string::npos) break;
      begin = end + 1;
    }
  }
  return {};
}

}  // namespace

BootApexJniResolution ResolveBootApexJniLibrary(
    const char* requested_soname, const char* caller_location,
    const char* android_filesystem_root) {
  BootApexJniResolution result;
  const std::string apex_module = ApexModule(caller_location);
  if (apex_module.empty()) return result;
  const std::string apex_namespace = ApexNamespace(apex_module);
  result.decision = BootApexJniDecision::kDenied;
  if (!IsSafeSoname(requested_soname)) {
    result.error = "APEX JNI request is not a safe Android SONAME";
    return result;
  }
  if (!IsAbsoluteWithoutTraversal(android_filesystem_root)) {
    result.error = "APEX JNI policy roots are missing or invalid";
    return result;
  }
  const std::string policy_path =
      std::string(android_filesystem_root) +
      "/linkerconfig/apex.libraries.config.txt";
  std::string policy;
  if (!ReadPolicy(policy_path, &policy, &result.error)) return result;
  if (!ContainsJniLibrary(policy, apex_namespace, requested_soname)) {
    result.error = "Android APEX namespace does not export requested JNI library";
    return result;
  }
  const std::string candidate =
      std::string(android_filesystem_root) + "/apex/" + apex_module +
      "/lib64/" + requested_soname;
  struct stat status {};
  if (lstat(candidate.c_str(), &status) != 0 || !S_ISREG(status.st_mode)) {
    result.error = "allowed Android APEX JNI library is absent from system image";
    return result;
  }
  result.decision = BootApexJniDecision::kAllowed;
  result.path = candidate;
  result.error.clear();
  return result;
}

std::string ResolvePublicApexLibrary(const char* soname,
                                     const char* android_filesystem_root) {
  if (!IsSafeSoname(soname) ||
      !IsAbsoluteWithoutTraversal(android_filesystem_root)) {
    return {};
  }
  std::string policy;
  std::string error;
  if (!ReadPolicy(std::string(android_filesystem_root) +
                      "/linkerconfig/apex.libraries.config.txt",
                  &policy, &error)) {
    return {};
  }
  std::string module = PublicApexNamespace(policy, soname);
  if (module.empty()) return {};
  for (char& byte : module) {
    if (byte == '_') byte = '.';
  }
  const std::string candidate =
      std::string(android_filesystem_root) + "/apex/" + module + "/lib64/" +
      soname;
  struct stat status {};
  return lstat(candidate.c_str(), &status) == 0 && S_ISREG(status.st_mode)
             ? candidate
             : std::string();
}

}  // namespace darwin_art::loader
