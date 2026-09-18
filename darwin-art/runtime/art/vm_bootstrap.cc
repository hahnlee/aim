#include "vm_bootstrap.h"

#include <mach-o/dyld.h>

#include <cerrno>
#include <cctype>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <iostream>
#include <sstream>
#include <string>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>

#include "../../compat/graphics/graphics_environment.h"
#include "../../compat/darwin_framework_natives.h"
#include "darwin_art/darwin_art.h"
#include "base/locks.h"
#include "base/logging.h"
#include "base/mem_map.h"
#include "cmdline_types.h"
#include "debugger.h"
#include "nativeloader/native_loader.h"
#include "parsed_options.h"
#include "runtime.h"
#include "runtime_options.h"

namespace darwin_art::runtime_art {
namespace {

bool ConfigureAndroidLogTags() {
  const char* tags = std::getenv("ANDROID_LOG_TAGS");
  if (tags == nullptr) return true;
  std::string value(tags);
  size_t start = 0;
  while (start < value.size()) {
    while (start < value.size() &&
           std::isspace(static_cast<unsigned char>(value[start]))) {
      ++start;
    }
    if (start == value.size()) break;
    const size_t end = value.find_first_of(" \t\r\n", start);
    const std::string spec = value.substr(start, end - start);
    if (spec.size() == 3 && spec[0] == '*' && spec[1] == ':') {
      using android::base::LogSeverity;
      LogSeverity severity;
      switch (spec[2]) {
        case 'v': severity = android::base::VERBOSE; break;
        case 'd': severity = android::base::DEBUG; break;
        case 'i': severity = android::base::INFO; break;
        case 'w': severity = android::base::WARNING; break;
        case 'e': severity = android::base::ERROR; break;
        case 'f':
        case 's': severity = android::base::FATAL_WITHOUT_ABORT; break;
        default: return false;
      }
      android::base::SetMinimumLogSeverity(severity);
    }
    start = end == std::string::npos ? value.size() : end + 1;
  }
  return true;
}

bool SetOptionalRuntimeFlags(art::RuntimeArgumentMap* options) {
  const char* serialized = std::getenv("DARWIN_ART_RUNTIME_OPTIONS");
  if (serialized != nullptr && serialized[0] != '\0') {
    std::vector<std::string> storage;
    std::istringstream stream(serialized);
    for (std::string option; std::getline(stream, option, '\n');) {
      if (!option.empty()) storage.push_back(std::move(option));
    }
    art::RuntimeOptions parsed;
    for (const std::string& option : storage) parsed.emplace_back(option, nullptr);
    if (!art::ParsedOptions::Parse(parsed, false, options)) return false;
  }
  return true;
}

bool ConfigureBootImage(const char* root, const std::string& boot_class_path,
                        art::RuntimeArgumentMap* options) {
  if (root == nullptr || root[0] == '\0') return true;
  const std::vector<std::string> bcp =
      art::ParseStringList<':'>::Split(boot_class_path);
  std::vector<int> image_fds(bcp.size(), -1);
  std::vector<int> vdex_fds(bcp.size(), -1);
  std::vector<int> oat_fds(bcp.size(), -1);
  for (size_t index = 0; index + 1 < bcp.size(); ++index) {
    std::string name = "boot";
    if (index != 0u) {
      const size_t slash = bcp[index].rfind('/');
      std::string base = bcp[index].substr(
          slash == std::string::npos ? 0u : slash + 1u);
      const size_t dot = base.rfind('.');
      if (dot != std::string::npos) base.resize(dot);
      name += "-" + base;
    }
    auto open_component = [&](const char* suffix) {
      const std::string path = std::string(root) + "/" + name + suffix;
      const int fd = open(path.c_str(), O_RDONLY | O_CLOEXEC);
      if (fd < 0) std::cerr << "ART runtime: missing boot image component "
                             << path << "\n";
      return fd;
    };
    image_fds[index] = open_component(".art");
    vdex_fds[index] = open_component(".vdex");
    oat_fds[index] = open_component(".oat");
    if (image_fds[index] < 0 || vdex_fds[index] < 0 || oat_fds[index] < 0)
      return false;
  }
  options->Set(art::RuntimeArgumentMap::Image,
               art::ParseStringList<':'>::Split("/system/framework/boot.art"));
  options->Set(art::RuntimeArgumentMap::BootClassPathImageFds,
               art::ParseIntList<':'> (std::move(image_fds)));
  options->Set(art::RuntimeArgumentMap::BootClassPathVdexFds,
               art::ParseIntList<':'> (std::move(vdex_fds)));
  options->Set(art::RuntimeArgumentMap::BootClassPathOatFds,
               art::ParseIntList<':'> (std::move(oat_fds)));
  return true;
}

}  // namespace

int CreateVm(const darwin_art_process_config_t* config,
             const embedding::ProcessConfigBounds& bounds,
             const std::string& application_class_path,
             VmBootstrapResult* result) {
  if (config == nullptr || result == nullptr) return 64;
  if (!ConfigureAndroidLogTags()) return 54;

  art::MemMap::Init();
  if (!darwin_art::InitializeFrameworkGraphicsRuntime()) return 36;

  std::string boot_class_path =
      std::string(config->core_oj_jar) + ":" + config->core_libart_jar + ":" +
      config->framework_jar + ":" + config->core_icu4j_jar;
  if (const char* configured = std::getenv("DARWIN_ART_BOOT_CLASSPATH");
      configured != nullptr && configured[0] != '\0') {
    boot_class_path = configured;
  }
  std::cerr << "Mach-O slide: 0x" << std::hex << _dyld_get_image_vmaddr_slide(0)
            << std::dec << "\n";
  art::Locks::Init();
  art::RuntimeArgumentMap options;
  if (!SetOptionalRuntimeFlags(&options)) return 55;
  options.Set(art::RuntimeArgumentMap::BootClassPath,
              art::ParseStringList<':'>::Split(boot_class_path));
  std::string locations = boot_class_path;
  if (const char* configured =
          std::getenv("DARWIN_ART_BOOT_CLASSPATH_LOCATIONS");
      configured != nullptr && configured[0] != '\0') {
    locations = configured;
  }
  options.Set(art::RuntimeArgumentMap::BootClassPathLocations,
              art::ParseStringList<':'>::Split(locations));
  if (!ConfigureBootImage(std::getenv("DARWIN_ART_BOOT_IMAGE_FD_ROOT"),
                          boot_class_path, &options)) return 56;

  options.Set(art::RuntimeArgumentMap::ClassPath, application_class_path);
  std::vector<std::string> properties{"java.class.path=" + application_class_path};
  if (const char* directory = std::getenv("DARWIN_ART_JAVA_IO_TMPDIR");
      directory != nullptr && directory[0] == '/') {
    properties.emplace_back(std::string("java.io.tmpdir=") + directory);
  }
  options.Set(art::RuntimeArgumentMap::PropertiesList, std::move(properties));

  bool enable_jit = options.Exists(art::RuntimeArgumentMap::UseJitCompilation)
                        ? options.GetOrDefault(art::RuntimeArgumentMap::UseJitCompilation)
                        : true;
  if (const char* jit = std::getenv("DARWIN_ART_JIT"); jit != nullptr) {
    if (std::strcmp(jit, "1") == 0) enable_jit = true;
    else if (std::strcmp(jit, "0") == 0) enable_jit = false;
    else return 55;
  }
  options.Set(art::RuntimeArgumentMap::UseJitCompilation, enable_jit);
  // app_process establishes target-SDK verifier policy before VM creation.
  // Preserve that specialization boundary for the unchanged installed APK.
  const auto read_optional_uint = [](const char* name, unsigned int* value) {
    const char* raw = std::getenv(name);
    if (raw == nullptr || *raw == '\0') return 0;
    if (!std::isdigit(static_cast<unsigned char>(*raw))) return -1;
    errno = 0;
    char* end = nullptr;
    const unsigned long parsed = std::strtoul(raw, &end, 10);
    if (errno != 0 || end == raw || *end != '\0' || parsed > UINT32_MAX)
      return -1;
    *value = static_cast<unsigned int>(parsed);
    return 1;
  };
  unsigned int value = 0;
  int status = read_optional_uint("DARWIN_ART_RUNTIME_TARGET_SDK_VERSION", &value);
  if (status < 0) return 49;
  if (status > 0) options.Set(art::RuntimeArgumentMap::TargetSdkVersion, value);
  status = read_optional_uint("DARWIN_ART_RUNTIME_FINALIZER_TIMEOUT_MS", &value);
  if (status < 0) return 52;
  if (status > 0) options.Set(art::RuntimeArgumentMap::FinalizerTimeoutMs, value);
  bool java_debuggable = false;
  if (const char* debuggable =
          std::getenv("DARWIN_ART_RUNTIME_JAVA_DEBUGGABLE");
      debuggable != nullptr) {
    if (std::strcmp(debuggable, "1") == 0) java_debuggable = true;
    else if (std::strcmp(debuggable, "0") != 0) return 50;
    if (java_debuggable) {
      auto compiler_options = options.GetOrDefault(
          art::RuntimeArgumentMap::CompilerOptions);
      compiler_options.emplace_back("--debuggable");
      options.Set(art::RuntimeArgumentMap::CompilerOptions,
                  std::move(compiler_options));
    }
  }
  if (!options.Exists(art::RuntimeArgumentMap::MemoryInitialSize))
    options.Set(art::RuntimeArgumentMap::MemoryInitialSize,
                art::MemoryKiB(bounds.heap_initial_bytes));
  if (!options.Exists(art::RuntimeArgumentMap::HeapGrowthLimit))
    options.Set(art::RuntimeArgumentMap::HeapGrowthLimit,
                art::MemoryKiB(bounds.heap_maximum_bytes));
  if (!options.Exists(art::RuntimeArgumentMap::MemoryMaximumSize))
    options.Set(art::RuntimeArgumentMap::MemoryMaximumSize,
                art::MemoryKiB(bounds.heap_maximum_bytes));
  art::LogVerbosity verbosity{};
  verbosity.heap = true;
  options.Set(art::RuntimeArgumentMap::Verbose, verbosity);
  if (!art::Runtime::Create(std::move(options))) return 1;

  android::InitializeNativeLoader();
  if (std::getenv("DARWIN_ART_UPSTREAM_JVMTI") != nullptr)
    art::Dbg::SetJdwpAllowed(true);
  if (java_debuggable)
    art::Runtime::Current()->SetRuntimeDebugState(
        art::Runtime::RuntimeDebugState::kJavaDebuggableAtInit);
  darwin_art::graphics::ConfigureGraphicsEnvironment(java_debuggable);

  result->self = art::Thread::Current();
  if (result->self == nullptr) return 2;
  result->env = reinterpret_cast<JNIEnv*>(result->self->GetJniEnv());
  result->jit_enabled = enable_jit;
  return 0;
}

}  // namespace darwin_art::runtime_art
