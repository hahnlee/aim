#include <dlfcn.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

#include "darwin_provider_owners.h"
#include "loader/boot_apex_jni_policy.h"
#include "loader/bionic_provider_set.h"
#include "loader/classloader_identity.h"
#include "loader/classloader_namespaces.h"
#include "loader/library_search.h"
#include "loader/guest_open_request.h"
#include "filesystem/process_authority.h"
#include "darwin_android_media_ndk.h"
#include "darwin_jni_shorty.h"
#include "darwin_art_bionic_builtin_adapters.h"
#include "darwin_runtime_adapters_internal.h"
#include "nativebridge/native_bridge.h"
#include "nativeloader/native_loader.h"

namespace android {
namespace {

std::mutex& ElfLibraryRegistryMutex() {
  static auto* mutex = new std::mutex();
  return *mutex;
}

std::vector<ElfLibrary*>& ElfLibraries() {
  static auto* libraries = new std::vector<ElfLibrary*>();
  return *libraries;
}

// Built-in NDK providers do not have a guest ELF image. Keep a typed sentinel
// for the media provider so bionic dlopen/dlsym still has a normal handle
// lifetime while symbol lookup remains owned by the runtime provider table.
int g_media_ndk_handle_tag = 0;

bool IsMediaNdkHandle(void* handle) {
  return handle == static_cast<void*>(&g_media_ndk_handle_tag);
}

// ElfLibraryRegistryMutex must be held. A Java ClassLoader can have many
// distinct global-reference values, but Android NativeLoader gives all of
// them one linker namespace. Prefer exact identity so detached native threads
// can recover their namespace without entering ART; use IsSameObject only
// while a live JNIEnv is available.
uint64_t FindLoaderNamespaceIdLocked(JNIEnv* env, jobject loader) {
  if (loader == nullptr) return 0;
  for (auto* library : ElfLibraries()) {
    if (library != nullptr && library->app_loader == loader) {
      return library->loader_namespace_id;
    }
  }
  return darwin_art::loader::FindClassLoaderIdentity(env, loader);
}

uint64_t GetOrCreateLoaderNamespaceId(JNIEnv* env, jobject loader) {
  return darwin_art::loader::EnsureClassLoaderIdentity(env, loader);
}


extern "C" uintptr_t darwin_art_bionic_rust_provider_closure_anchor();

void SetNativeLoaderError(char** error_msg, const std::string& message) {
  if (error_msg != nullptr) *error_msg = strdup(message.c_str());
}


// Guest libdl calls made by an Android ELF image must stay in the same
// NativeLoader namespace as the image that issued the call.  The dso facade
// exposes a process callback ABI, so bind it to the first app image and route
// every subsequent open/lookup/close through the existing ART loader seam.
void CopyLoaderError(const std::string& message, char* out, size_t capacity) {
  if (out == nullptr || capacity == 0) return;
  std::strncpy(out, message.c_str(), capacity - 1);
  out[capacity - 1] = '\0';
}

// bionic dlopen itself does not require an Android native worker to remain
// attached to the VM. Our ELF graph construction does need a JNIEnv briefly
// because it retains the initiating ClassLoader and installs the JNI proxy.
// Attach only for that internal bookkeeping interval and restore the caller's
// original detached state before returning to guest code.
class ScopedGuestLoaderAttachment {
 public:
  explicit ScopedGuestLoaderAttachment(ElfLibrary* owner)
      : vm_(owner == nullptr ? nullptr : owner->art_vm), env_(CurrentArtEnv()) {
    if (env_ != nullptr || vm_ == nullptr) return;
    void* current = nullptr;
    const jint state = vm_->GetEnv(&current, JNI_VERSION_1_6);
    if (state == JNI_OK) {
      env_ = static_cast<JNIEnv*>(current);
      return;
    }
    if (state == JNI_EDETACHED &&
        vm_->AttachCurrentThreadAsDaemon(&env_, nullptr) == JNI_OK) {
      attached_here_ = true;
    }
  }

  ~ScopedGuestLoaderAttachment() {
    if (attached_here_) (void)vm_->DetachCurrentThread();
  }

  ScopedGuestLoaderAttachment(const ScopedGuestLoaderAttachment&) = delete;
  ScopedGuestLoaderAttachment& operator=(const ScopedGuestLoaderAttachment&) =
      delete;

  JNIEnv* env() const { return env_; }
  bool attached_here() const { return attached_here_; }

 private:
  JavaVM* vm_ = nullptr;
  JNIEnv* env_ = nullptr;
  bool attached_here_ = false;
};

void* GuestDsoOpen(void* context, const char* path, int flags, const DarwinArtAndroidDlExtInfo* info,
                  char* error, size_t capacity) {
  if (const char* rejection = darwin_art::loader::LegacyOpenRequestError(flags, info)) {
    CopyLoaderError(rejection, error, capacity);
    return nullptr;
  }
  auto* owner = static_cast<ElfLibrary*>(context);
  if (owner == nullptr || owner->magic != kElfLibraryMagic || path == nullptr) {
    CopyLoaderError("invalid Android guest loader context", error, capacity);
    return nullptr;
  }
  if (std::strcmp(path, "libmediandk.so") == 0) {
    if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr)
      std::fprintf(stderr, "DARWIN guest libdl open builtin path=%s\n", path);
    return static_cast<void*>(&g_media_ndk_handle_tag);
  }
  // bionic dlopen is identity based: a second open of an already loaded
  // absolute SONAME returns the original handle and does not invoke
  // JNI_OnLoad again. Unity opens libil2cpp once through NativeLoader and
  // again through its internal libdl path, so preserve that contract here.
  if (ElfLibrary* existing = FindElfLibraryByPath(
          CurrentArtEnv(), path, static_cast<jobject>(owner->app_loader));
      existing != nullptr && existing->graph_handle != nullptr) {
    existing->guest_open_refs.fetch_add(1, std::memory_order_relaxed);
    if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
      std::fprintf(stderr,
                   "DARWIN guest libdl open reuse path=%s handle=%p refs=%u\n",
                   path, existing->graph_handle,
                   existing->guest_open_refs.load(std::memory_order_relaxed));
    }
    return existing->graph_handle;
  }
  ScopedGuestLoaderAttachment attachment(owner);
  if (attachment.env() == nullptr) {
    CopyLoaderError("Android guest ELF loader could not attach native worker",
                    error, capacity);
    return nullptr;
  }
  if (attachment.attached_here() &&
      std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN guest libdl temporary ART attach path=%s\n", path);
  }
  bool needs_bridge = false;
  char* native_error = nullptr;
  void* handle = OpenNativeLibrary(attachment.env(), 35, path,
                                   static_cast<jobject>(owner->app_loader), nullptr,
                                   nullptr, &needs_bridge, &native_error);
  if (handle == nullptr || !needs_bridge) {
    const std::string message = native_error == nullptr
                                    ? "Android guest ELF open failed"
                                    : native_error;
    CopyLoaderError(message, error, capacity);
    if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
      std::fprintf(stderr, "DARWIN guest libdl open failure path=%s error=%s\n",
                   path, message.c_str());
    }
  }
  NativeLoaderFreeErrorMessage(native_error);
  if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
    std::fprintf(stderr, "DARWIN guest libdl open path=%s handle=%p bridge=%d\n",
                 path, handle, needs_bridge ? 1 : 0);
  }
  return handle;
}

void* GuestDsoLookup(void*, void* handle, const char* symbol, const char*,
                    char* error, size_t capacity) {
  if (IsMediaNdkHandle(handle)) {
    void* address = darwin_art_android_media_ndk_symbol(symbol);
    if (address != nullptr) return address;
    CopyLoaderError("Android media NDK symbol is unavailable", error, capacity);
    return nullptr;
  }
  auto* owner = AsElfLibrary(handle);
  uintptr_t address = 0;
  std::string lookup_error;
  if (owner == nullptr || symbol == nullptr ||
      !LookupOptionalElfSymbol(owner, symbol, &address, &lookup_error) || address == 0) {
    if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
      std::fprintf(stderr,
                   "DARWIN guest libdl lookup handle=%p symbol=%s result=null error=%s\n",
                   handle, symbol == nullptr ? "(null)" : symbol,
                   lookup_error.c_str());
    }
    CopyLoaderError(lookup_error.empty() ? "Android guest ELF symbol lookup failed"
                                         : lookup_error,
                    error, capacity);
    return nullptr;
  }
  if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN guest libdl lookup handle=%p symbol=%s result=%p\n",
                 handle, symbol, reinterpret_cast<void*>(address));
  }
  return reinterpret_cast<void*>(address);
}

int GuestDsoClose(void*, void* handle, char* error, size_t capacity) {
  if (IsMediaNdkHandle(handle)) return 0;
  if (ElfLibrary* library = AsElfLibrary(handle);
      library != nullptr &&
      library->guest_open_refs.load(std::memory_order_relaxed) != 0) {
    library->guest_open_refs.fetch_sub(1, std::memory_order_relaxed);
    return 0;
  }
  char* native_error = nullptr;
  const bool closed = CloseNativeLibrary(handle, true, &native_error);
  if (!closed) {
    CopyLoaderError(native_error == nullptr ? "Android guest ELF close failed"
                                            : native_error,
                    error, capacity);
  }
  NativeLoaderFreeErrorMessage(native_error);
  return closed ? 0 : -1;
}

void BindGuestDsoLoader(ElfLibrary* library) {
  if (library == nullptr || std::getenv("DARWIN_ART_APK_APP_NATIVE_PATH") == nullptr) return;
  DarwinArtLoaderCallbacks callbacks{library, &GuestDsoOpen, &GuestDsoLookup,
                                     &GuestDsoClose};
  if (darwin_art_loader_bind(&callbacks) != 0) {
    if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
      std::fprintf(stderr, "DARWIN guest libdl: loader namespace already bound\n");
    }
  } else {
    if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
      std::fprintf(stderr, "DARWIN guest libdl: bound app ELF namespace\n");
    }
  }
}

std::string ElfError(DarwinArtElfStatus status,
                     const DarwinArtElfErrorBuffer& error) {
  std::ostringstream stream;
  stream << darwin_art_elf_status_name(status);
  if (error.data != nullptr && error.data[0] != '\0') {
    stream << ": " << error.data;
  }
  return stream.str();
}

bool SplitTrustedLibraryPath(const char* path,
                             std::string* parent,
                             std::vector<uint8_t>* component) {
  if (path == nullptr || path[0] == '\0') return false;
  const std::string bytes(path);
  const size_t slash = bytes.find_last_of('/');
  const size_t component_offset = slash == std::string::npos ? 0 : slash + 1;
  if (component_offset == bytes.size()) return false;
  *parent = slash == std::string::npos ? "." :
            slash == 0 ? "/" : bytes.substr(0, slash);
  component->assign(bytes.begin() + component_offset, bytes.end());
  return true;
}

void* OpenSelectedDarwinArtifact(const char* requested_path,
                                 bool* selected,
                                 char** error_msg) {
  *selected = false;
  const char* backend = std::getenv("DARWIN_ART_APK_NATIVE_BACKEND");
  if (backend == nullptr || std::strcmp(backend, "darwin") != 0) return nullptr;
  *selected = true;
  const char* directory = std::getenv("DARWIN_ART_APK_DARWIN_DIRECTORY");
  if (requested_path == nullptr || directory == nullptr || directory[0] != '/') {
    SetNativeLoaderError(error_msg,
                         "Rust selected Darwin native graph without an absolute directory");
    return nullptr;
  }
  std::string ignored_parent;
  std::vector<uint8_t> component;
  if (!SplitTrustedLibraryPath(requested_path, &ignored_parent, &component) ||
      component.size() <= 3 ||
      std::memcmp(component.data() + component.size() - 3, ".so", 3) != 0) {
    SetNativeLoaderError(error_msg,
                         "Darwin native request has no direct Android SONAME");
    return nullptr;
  }
  std::string leaf(component.begin(), component.end() - 3);
  if (leaf.find('/') != std::string::npos || leaf.find('\\') != std::string::npos) {
    SetNativeLoaderError(error_msg, "Darwin native request has an unsafe SONAME");
    return nullptr;
  }
  const std::string dylib_path = std::string(directory) + "/" + leaf + ".dylib";
  // Each Darwin host process owns exactly one Android application native
  // namespace. Promote translated Mach-O symbols within that process so
  // Bionic-style RTLD_DEFAULT lookups from the DSO can resolve namespace
  // peers (and the DSO itself); process isolation still prevents cross-app
  // symbol leakage.
  void* handle = dlopen(dylib_path.c_str(), RTLD_NOW | RTLD_GLOBAL);
  if (handle == nullptr) {
    const char* message = dlerror();
    SetNativeLoaderError(
        error_msg,
        message == nullptr ? "complete Darwin native graph failed to load" : message);
    return nullptr;
  }
  if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
    std::fprintf(stderr, "DARWIN native loader: complete graph root=%s\n",
                 dylib_path.c_str());
  }
  return handle;
}

struct DiscoveredGraphDeleter {
  void operator()(DarwinArtElfDiscoveredGraph* graph) const {
    darwin_art_elf_discovered_graph_destroy(&graph);
  }
};

class ScopedFd {
 public:
  ScopedFd() = default;
  explicit ScopedFd(int fd) : fd_(fd) {}
  ~ScopedFd() { if (fd_ >= 0) close(fd_); }
  ScopedFd(const ScopedFd&) = delete;
  ScopedFd& operator=(const ScopedFd&) = delete;
  ScopedFd(ScopedFd&& other) noexcept : fd_(std::exchange(other.fd_, -1)) {}
  ScopedFd& operator=(ScopedFd&& other) noexcept {
    if (this != &other) {
      if (fd_ >= 0) close(fd_);
      fd_ = std::exchange(other.fd_, -1);
    }
    return *this;
  }
  int get() const { return fd_; }

 private:
  int fd_ = -1;
};

bool AttachNativeOwner(ElfLibrary* library,
                       uint32_t order,
                       void* value,
                       RuntimeNativeOwnerDropFn drop_fn,
                       std::string* error) {
  if (library == nullptr || library->native_owner == nullptr || value == nullptr ||
      darwin_art_runtime_native_owner_attach(library->native_owner, order, value,
                                              library, drop_fn) != 0) {
    if (value != nullptr && drop_fn != nullptr) {
      (void)drop_fn(value, library);
    }
    if (error != nullptr) *error = "Rust native owner rejected resource slot";
    return false;
  }
  return true;
}

bool LoadAndroidUnwindProvider(ElfLibrary* library, std::string* error) {
  const char* path = std::getenv("DARWIN_ART_ANDROID_UNWIND_PROVIDER");
  if (path == nullptr || path[0] == '\0') return true;
  if (path[0] != '/') {
    *error = "Android unwind provider path is not absolute";
    return false;
  }
  DarwinArtElfLoadOptions options{DARWIN_ART_ELF_ABI_VERSION,
                                  &ResolveRuntimeProvider, library};
  std::array<char, 1024> storage{};
  DarwinArtElfErrorBuffer error_buffer{storage.data(), storage.size(), 0};
  DarwinArtElfHandle* handle = nullptr;
  DarwinArtElfStatus status =
      darwin_art_elf_load_path(path, &options, &handle, &error_buffer);
  if (status == DARWIN_ART_ELF_OK) {
    status = darwin_art_elf_run_initializers(handle, &error_buffer);
  }
  if (status != DARWIN_ART_ELF_OK) {
    if (handle != nullptr) {
      std::array<char, 256> unload_storage{};
      DarwinArtElfErrorBuffer unload_error{unload_storage.data(),
                                            unload_storage.size(), 0};
      (void)darwin_art_elf_unload(&handle, &unload_error);
    }
    *error = "Android unwind provider load failed: " +
             ElfError(status, error_buffer);
    return false;
  }
  library->android_unwind_provider = handle;
  if (!AttachNativeOwner(library, kNativeOwnerAndroidUnwind, handle,
                         &DropRuntimeAndroidUnwindProvider, error)) {
    return false;
  }
  return true;
}

}  // namespace

void RegisterElfLibrary(ElfLibrary* library) {
  if (library == nullptr) return;
  std::lock_guard<std::mutex> lock(ElfLibraryRegistryMutex());
  auto& libraries = ElfLibraries();
  if (std::find(libraries.begin(), libraries.end(), library) == libraries.end()) {
    libraries.push_back(library);
  }
}

ElfLibrary* FindElfLibraryByPath(JNIEnv* env, const char* path, jobject loader) {
  if (path == nullptr || path[0] == '\0') return nullptr;
  std::lock_guard<std::mutex> lock(ElfLibraryRegistryMutex());
  const uint64_t namespace_id = FindLoaderNamespaceIdLocked(env, loader);
  if (loader != nullptr && namespace_id == 0) return nullptr;
  for (auto* library : ElfLibraries()) {
    if (library == nullptr || library->resolved_path != path) continue;
    if (library->loader_namespace_id == namespace_id) return library;
  }
  return nullptr;
}

// A few Unity worker paths call bionic dlopen after detaching from the Java
// VM.  The resident ART image is still the authoritative owner in that case;
// an exact resolved path is sufficient to lease it without manufacturing a
// second ELF graph (or requiring a JNIEnv that the caller intentionally does
// not have).
ElfLibrary* FindResidentElfLibraryByPath(const char* path) {
  if (path == nullptr || path[0] == '\0') return nullptr;
  std::lock_guard<std::mutex> lock(ElfLibraryRegistryMutex());
  for (auto* library : ElfLibraries()) {
    if (library != nullptr && library->resolved_path == path &&
        library->graph_handle != nullptr) {
      return library;
    }
  }
  return nullptr;
}

void UnregisterElfLibrary(ElfLibrary* library) {
  if (library == nullptr) return;
  std::lock_guard<std::mutex> lock(ElfLibraryRegistryMutex());
  auto& libraries = ElfLibraries();
  libraries.erase(std::remove(libraries.begin(), libraries.end(), library),
                  libraries.end());
}

bool ShutdownElfLibraries() {
  // DropRuntimeElfLibrary unregisters each image as it is destroyed, so take
  // a snapshot and never hold the registry mutex across callbacks.
  std::vector<void*> handles;
  {
    std::lock_guard<std::mutex> lock(ElfLibraryRegistryMutex());
    for (ElfLibrary* library : ElfLibraries()) {
      if (library != nullptr && library->graph_handle != nullptr) {
        handles.push_back(library->graph_handle);
      }
    }
  }
  for (void* handle : handles) {
    char* error = nullptr;
    if (!CloseNativeLibrary(handle, true, &error)) {
      if (error != nullptr) NativeLoaderFreeErrorMessage(error);
      return false;
    }
  }
  return true;
}

ElfLibrary* FindElfLibraryForAddress(uintptr_t address) {
  if (address == 0) return nullptr;
  std::lock_guard<std::mutex> lock(ElfLibraryRegistryMutex());
  for (auto* library : ElfLibraries()) {
    if (library != nullptr && library->image_registry != nullptr &&
        darwin_art_image_registry::ContainsAddress(library->image_registry,
                                                    address)) {
      return library;
    }
  }
  return nullptr;
}

ElfLibrary* AsElfLibrary(void* handle) {
  auto* library = static_cast<ElfLibrary*>(
      darwin_art_runtime_native_owner_lookup(
          static_cast<RuntimeNativeOwner*>(handle), kNativeOwnerGraphHandle));
  return library != nullptr && library->magic == kElfLibraryMagic ? library : nullptr;
}

extern "C" void* OpenNativeLibrary(JNIEnv* env,
                                    int32_t,
                                    const char* path,
                                    jobject loader,
                                    const char* caller_location,
                                    jstring library_path,
                                    bool* needs_native_bridge,
                                    char** error_msg) {
  if (needs_native_bridge != nullptr) *needs_native_bridge = false;
  if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN native loader request path=%s loader=%p caller=%s\n",
                 path == nullptr ? "(null)" : path, loader,
                 caller_location == nullptr ? "(null)" : caller_location);
  }
  // Boot-classpath modules (for example Conscrypt) resolve JNI from their
  // APEX namespace, not from an APK's nativeLibraryPath.  NativeLoader gives
  // those requests a null class loader and may pass only the SONAME.  Map that
  // narrow case into the immutable Android system-native directory so it can
  // use the same fd-relative ELF graph discovery as APK libraries.
  std::string system_native_path;
  std::string app_native_path;
  const char* resolved_path = path;
  if (path != nullptr && std::strchr(path, '/') == nullptr &&
      std::strchr(path, '\\') == nullptr) {
    const auto apex = darwin_art::loader::ResolveBootApexJniLibrary(
        path, caller_location,
        std::getenv("DARWIN_ART_ANDROID_FILESYSTEM_ROOT"));
    if (apex.decision == darwin_art::loader::BootApexJniDecision::kDenied) {
      SetNativeLoaderError(error_msg, apex.error);
      return nullptr;
    }
    if (apex.decision == darwin_art::loader::BootApexJniDecision::kAllowed) {
      system_native_path = apex.path;
      resolved_path = system_native_path.c_str();
    }
  }
  if (loader == nullptr && path != nullptr && std::strchr(path, '/') == nullptr &&
      std::strchr(path, '\\') == nullptr && system_native_path.empty()) {
    const char* system_directory =
        std::getenv("DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR");
    if (system_directory != nullptr && system_directory[0] == '/' &&
        std::strstr(path, "..") == nullptr) {
      const std::string candidate = std::string(system_directory) + "/" + path;
      struct stat candidate_status {};
      if (stat(candidate.c_str(), &candidate_status) == 0 &&
          S_ISREG(candidate_status.st_mode)) {
        system_native_path = candidate;
        resolved_path = system_native_path.c_str();
      }
    }
  }
  // A null ClassLoader is also used by Android's process-native dlopen path.
  // If that path names an exact image already resident in this process, bionic
  // returns the same handle and bumps its reference count; it does not create
  // a second copy merely because a live JNIEnv happens to be present.  The
  // absolute path is the retained NativeLoader identity when no Java loader
  // namespace is supplied.
  if (env == nullptr || loader == nullptr) {
    if (ElfLibrary* existing = FindResidentElfLibraryByPath(resolved_path);
        existing != nullptr) {
      existing->guest_open_refs.fetch_add(1, std::memory_order_relaxed);
      if (needs_native_bridge != nullptr) *needs_native_bridge = true;
      if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
        std::fprintf(stderr,
                     "DARWIN native loader: path reuse path=%s handle=%p refs=%u "
                     "detached=%d\n",
                     resolved_path, existing->graph_handle,
                     existing->guest_open_refs.load(std::memory_order_relaxed),
                     env == nullptr ? 1 : 0);
      }
      return existing->graph_handle;
    }
  }
  // Framework media classes load libmedia_jni only to install their JNI
  // table. The Darwin runtime registers the deliberately unsupported MediaDrm
  // table during framework bootstrap, so publish the already-loaded process
  // image for this exact boot-classpath SONAME instead of requiring an Android
  // media server DSO in the guest filesystem.
  if (loader == nullptr && path != nullptr &&
      std::strcmp(path, "libmedia_jni.so") == 0) {
    void* handle = dlopen(nullptr, RTLD_NOW | RTLD_LOCAL);
    if (handle == nullptr) {
      SetNativeLoaderError(error_msg,
                           "Darwin platform media JNI image is unavailable");
    } else {
      if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
        std::fprintf(stderr,
                     "DARWIN native loader: platform JNI SONAME=%s provider=runtime\n",
                     path);
      }
    }
    return handle;
  }
  if (path != nullptr && std::strcmp(path, "libmediandk.so") == 0) {
    if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr)
      std::fprintf(stderr, "DARWIN native loader: builtin SONAME=%s\n", path);
    return static_cast<void*>(&g_media_ndk_handle_tag);
  }
  // Android's NativeLoader searches the ClassLoader namespace when
  // Runtime.loadLibrary0 deliberately falls back to a bare SONAME.  There is
  // no bionic linker namespace on Darwin, so resolve that SONAME against the
  // exact library search path JavaVMExt obtained from this ClassLoader before
  // entering the fd-relative ELF loader.
  if (loader != nullptr && path != nullptr && std::strchr(path, '/') == nullptr &&
      std::strchr(path, '\\') == nullptr && std::strstr(path, "..") == nullptr) {
    std::string namespace_error;
    if (!darwin_art::loader::FindClassLoaderLibrary(
            env, loader, path, &app_native_path, &namespace_error)) {
      SetNativeLoaderError(error_msg, namespace_error);
      return nullptr;
    }
    resolved_path = app_native_path.c_str();
  }
  // A native worker may invoke bionic dlopen after detaching from ART. Reuse
  // remains possible without entering ART. New images arrive here with the
  // short-lived JNIEnv established by GuestDsoOpen; JNI_OnLoad is a separate
  // JavaVMExt/NativeBridge operation and is intentionally not invoked here.
  if (env == nullptr && resolved_path != nullptr) {
    if (ElfLibrary* existing = FindElfLibraryByPath(nullptr, resolved_path, loader);
        existing != nullptr && existing->graph_handle != nullptr) {
      existing->guest_open_refs.fetch_add(1, std::memory_order_relaxed);
      if (needs_native_bridge != nullptr) *needs_native_bridge = true;
      if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
        std::fprintf(stderr,
                     "DARWIN native loader detached reuse path=%s handle=%p "
                     "namespace=%llu refs=%u\n",
                     resolved_path, existing->graph_handle,
                     static_cast<unsigned long long>(
                         existing->loader_namespace_id),
                     existing->guest_open_refs.load(std::memory_order_relaxed));
      }
      return existing->graph_handle;
    }
  }
  // An unresolved application SONAME must not escape into dyld's process-wide
  // search. Public namespace links must resolve it explicitly before this point.
  if (loader != nullptr && resolved_path != nullptr &&
      std::strchr(resolved_path, '/') == nullptr) {
    SetNativeLoaderError(error_msg,
                         std::string("Library not found in ClassLoader search path: ") +
                             resolved_path);
    return nullptr;
  }
  bool selected_darwin = false;
  void* selected_handle =
      system_native_path.empty()
          ? OpenSelectedDarwinArtifact(resolved_path, &selected_darwin, error_msg)
          : nullptr;
  if (selected_darwin) return selected_handle;
  const uint64_t loader_namespace_id = GetOrCreateLoaderNamespaceId(env, loader);
  if (loader != nullptr && loader_namespace_id == 0) {
    SetNativeLoaderError(error_msg, "ClassLoader identity allocation failed");
    return nullptr;
  }
  const char* builtin_providers[] = {
      kDarwinArtElfJniHostProviderSoname,
      "libc.so",
      "libdl.so",
      "liblog.so",
      "libm.so",
      "libandroid.so",
      "libbinder_ndk.so",
      "libaaudio.so",
      "libmediandk.so",
      "libnativewindow.so",
      "libEGL.so",
      "libGLESv2.so",
      "libGLESv3.so",
      "libjnigraphics.so",
      "libz.so"};
  std::array<char, 1024> discovery_error_storage{};
  DarwinArtElfErrorBuffer discovery_error{discovery_error_storage.data(),
                                           discovery_error_storage.size(), 0};
  DarwinArtElfDiscoveredGraph* discovered_raw = nullptr;
  DarwinArtElfStatus discovery_status = DARWIN_ART_ELF_INVALID_ARGUMENT;
  int root_is_elf = 0;
  std::string parent_path;
  std::vector<uint8_t> root_component;
  darwin_art::filesystem::ProcessAuthorityLease filesystem_lease;
  if (SplitTrustedLibraryPath(resolved_path, &parent_path, &root_component)) {
    std::vector<std::string> cached_sonames = SnapshotCachedElfSonames(loader_namespace_id);
    std::vector<const char*> providers(std::begin(builtin_providers),
                                       std::end(builtin_providers));
    const std::string requested_component(root_component.begin(),
                                          root_component.end());
    for (const std::string& soname : cached_sonames) {
      if (soname != requested_component) providers.push_back(soname.c_str());
    }
    ScopedFd opened_directory(open(parent_path.c_str(),
                                   O_RDONLY | O_DIRECTORY | O_CLOEXEC |
                                       O_NOFOLLOW));
    if (opened_directory.get() < 0) {
      discovery_status = DARWIN_ART_ELF_IO;
    } else {
      ScopedFd root_file(openat(opened_directory.get(), requested_component.c_str(),
                               O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK));
      std::array<unsigned char, 4> magic{};
      ssize_t magic_size = -1;
      if (root_file.get() >= 0) {
        do {
          magic_size = pread(root_file.get(), magic.data(), magic.size(), 0);
        } while (magic_size < 0 && errno == EINTR);
        if (magic_size < 0) {
          SetNativeLoaderError(error_msg, "native image header read failed");
          return nullptr;
        }
      }
      const bool is_elf = magic_size == 4 &&
          magic == std::array<unsigned char, 4>{0x7f, 'E', 'L', 'F'};
      if (is_elf) {
        std::string error;
        if (!filesystem_lease.Acquire(
                std::getenv("DARWIN_ART_ANDROID_FILESYSTEM_ROOT"), &error)) {
          SetNativeLoaderError(error_msg, "Bionic filesystem setup failed: " + error);
          return nullptr;
        }
      }
      struct DependencyContext {
        int directory_fd;
        const char* filesystem_root;
      } dependency_context{
          opened_directory.get(),
          std::getenv("DARWIN_ART_ANDROID_FILESYSTEM_ROOT")};
      // Existing closed-directory policy, pending namespace-owned admission.
      // Root is already open and is never reopened during graph discovery.
      auto open_dependency = +[](void* context, uint64_t, const char*,
                                 const char* name, const char*, uint64_t* image) -> int {
        *image = 0;
        auto* dependency = static_cast<DependencyContext*>(context);
        int descriptor =
            openat(dependency->directory_fd, name,
                   O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
        if (descriptor >= 0) return descriptor;
        const std::string exported = darwin_art::loader::ResolvePublicApexLibrary(
            name, dependency->filesystem_root);
        return exported.empty()
                   ? -1
                   : open(exported.c_str(),
                          O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
      };
      discovery_status = darwin_art_elf_discover_admitted_graph(
          root_file.get(), 0, root_component.data(), root_component.size(),
          providers.data(), providers.size(), open_dependency, &dependency_context,
          &root_is_elf, &discovered_raw, &discovery_error);
    }
  }
  std::unique_ptr<DarwinArtElfDiscoveredGraph, DiscoveredGraphDeleter> discovered(
      discovered_raw);
  if (discovery_status == DARWIN_ART_ELF_OK) {
    if (env == nullptr) {
      SetNativeLoaderError(error_msg, "Android ELF load requires a live JNIEnv");
      return nullptr;
    }
    const char* root_soname = nullptr;
    const DarwinArtElfGraphSource* sources = nullptr;
    size_t source_count = 0;
    DarwinArtElfStatus status = darwin_art_elf_discovered_graph_root_soname(
        discovered.get(), &root_soname, &discovery_error);
    if (status == DARWIN_ART_ELF_OK) {
      status = darwin_art_elf_discovered_graph_sources(
          discovered.get(), &sources, &source_count, &discovery_error);
    }
    if (status != DARWIN_ART_ELF_OK || root_soname == nullptr || sources == nullptr ||
        source_count == 0) {
      SetNativeLoaderError(error_msg,
                           "Android ELF discovery produced an invalid graph view");
      return nullptr;
    }
    std::string error;
    auto library = std::make_unique<ElfLibrary>();
    library->resolved_path = resolved_path == nullptr ? "" : resolved_path;
    library->cached_root_soname = root_soname;
    library->loader_namespace_id = loader_namespace_id;
    library->app_loader = loader == nullptr ? nullptr : env->NewGlobalRef(loader);
    if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
      std::fprintf(stderr, "DARWIN ELF loader: initiating loader=%p global=%p\n",
                   loader, library->app_loader);
    }
    library->native_owner = darwin_art_runtime_native_owner_create();
    if (library->native_owner == nullptr) {
      SetNativeLoaderError(error_msg, "Rust native owner allocation failed");
      return nullptr;
    }
    library->fixture_graph = IsExactFixtureGraph(root_soname, sources, source_count);
    if (library->fixture_graph) {
      g_elf_fixture_lifecycle.store(0, std::memory_order_relaxed);
      g_elf_fixture_provider_routes.store(0, std::memory_order_relaxed);
      g_elf_fixture_namespace_lifecycle.store(0, std::memory_order_relaxed);
    }
    if (darwin_art_bionic_rust_provider_closure_anchor() == 0) {
      SetNativeLoaderError(error_msg, "Bionic Rust provider closure is unavailable");
      return nullptr;
    }
    library->dso_lifecycle = darwin_art_bionic_dso_lifecycle_owner_create();
    if (library->dso_lifecycle == nullptr) {
      SetNativeLoaderError(error_msg, "Bionic DSO lifecycle activation failed");
      return nullptr;
    }
    if (!AttachNativeOwner(library.get(), kNativeOwnerDso, library->dso_lifecycle,
                           &DropRuntimeDsoLifecycle, &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner DSO slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    std::vector<std::string> cached_sonames = SnapshotCachedElfSonames(loader_namespace_id);
    std::vector<const char*> graph_providers(std::begin(builtin_providers),
                                             std::end(builtin_providers));
    for (const std::string& soname : cached_sonames) {
      if (soname != root_soname) graph_providers.push_back(soname.c_str());
    }
    library->image_registry = darwin_art_image_registry::Create(
        root_soname, parent_path.c_str(), sources, source_count, graph_providers.data(),
        graph_providers.size(), &error);
    if (library->image_registry == nullptr) {
      SetNativeLoaderError(error_msg,
                           "Android ELF image registry setup failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    RegisterElfLibrary(library.get());
    if (!AttachNativeOwner(library.get(), kNativeOwnerImageRegistry,
                           library->image_registry, &DropRuntimeElfImageRegistry,
                           &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner image slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    auto providers = darwin_art::loader::CreateBionicProviderSet(&error);
    if (!providers) {
      SetNativeLoaderError(error_msg, error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    // AttachNativeOwner consumes the resource on both success and failure.
    auto* provider_namespace = providers.release();
    if (!AttachNativeOwner(library.get(), kNativeOwnerNamespace,
                           provider_namespace, &DropRuntimeProviderNamespace,
                           &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner namespace slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    library->provider_namespace = provider_namespace;
    if (library->fixture_graph) {
      g_elf_fixture_namespace_lifecycle.store(1, std::memory_order_relaxed);
    }
    if (library->fixture_graph) {
      g_elf_fixture_namespace_lifecycle.store(2, std::memory_order_relaxed);
    }
    if (!darwin_art::providers::acquire_vm(&error)) {
      SetNativeLoaderError(error_msg, "Bionic VM setup failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!AttachNativeOwner(
            library.get(), kNativeOwnerVm,
            reinterpret_cast<void*>(
                static_cast<uintptr_t>(darwin_art::providers::Kind::Vm)),
            &DropRuntimeProviderKind, &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner VM slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    // AttachNativeOwner consumes this lease even if attaching fails.
    filesystem_lease.Transfer();
    if (!AttachNativeOwner(
            library.get(), kNativeOwnerFilesystem,
            reinterpret_cast<void*>(static_cast<uintptr_t>(
                darwin_art::providers::Kind::Filesystem)),
            &DropRuntimeProviderKind, &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner filesystem slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!darwin_art::providers::acquire_sendfile(&error)) {
      SetNativeLoaderError(error_msg, "Bionic sendfile setup failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!AttachNativeOwner(
            library.get(), kNativeOwnerSendfile,
            reinterpret_cast<void*>(static_cast<uintptr_t>(
                darwin_art::providers::Kind::Sendfile)),
            &DropRuntimeProviderKind, &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner sendfile slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!darwin_art::providers::acquire_ioctl(&error)) {
      SetNativeLoaderError(error_msg, "Bionic ioctl setup failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!AttachNativeOwner(
            library.get(), kNativeOwnerIoctl,
            reinterpret_cast<void*>(static_cast<uintptr_t>(
                darwin_art::providers::Kind::Ioctl)),
            &DropRuntimeProviderKind, &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner ioctl slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!darwin_art::providers::acquire_strftime(&error)) {
      SetNativeLoaderError(error_msg, "Bionic strftime setup failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!AttachNativeOwner(
            library.get(), kNativeOwnerStrftime,
            reinterpret_cast<void*>(static_cast<uintptr_t>(
                darwin_art::providers::Kind::Strftime)),
            &DropRuntimeProviderKind, &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner strftime slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!darwin_art::providers::acquire_stdio(&error)) {
      SetNativeLoaderError(error_msg, "Bionic stdio setup failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!AttachNativeOwner(
            library.get(), kNativeOwnerStdio,
            reinterpret_cast<void*>(static_cast<uintptr_t>(
                darwin_art::providers::Kind::Stdio)),
            &DropRuntimeProviderKind, &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner stdio slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!darwin_art::providers::acquire_network(&error)) {
      SetNativeLoaderError(error_msg, "Bionic network setup failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!AttachNativeOwner(
            library.get(), kNativeOwnerNetwork,
            reinterpret_cast<void*>(static_cast<uintptr_t>(
                darwin_art::providers::Kind::Network)),
            &DropRuntimeProviderKind, &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner network slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!LoadAndroidUnwindProvider(library.get(), &error)) {
      SetNativeLoaderError(error_msg, error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    DarwinArtElfLoadOptions options{DARWIN_ART_ELF_ABI_VERSION,
                                    &ResolveRuntimeProvider, library.get()};
    DarwinArtElfLifecycleCallbacks lifecycle{DARWIN_ART_ELF_ABI_VERSION,
                                             &PublishRuntimeElfImage,
                                             &FinalizeRuntimeElfImage,
                                             library.get()};
    std::array<char, 1024> error_storage{};
    DarwinArtElfErrorBuffer error_buffer{error_storage.data(), error_storage.size(), 0};
    status = darwin_art_elf_graph_load_with_lifecycle(
        root_soname, sources, source_count, graph_providers.data(),
        graph_providers.size(),
        &options, &lifecycle, &library->graph, &error_buffer);
    if (status != DARWIN_ART_ELF_OK) {
      SetNativeLoaderError(error_msg,
                           "Android ELF graph load failed: " + ElfError(status, error_buffer));
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
      std::fprintf(stderr, "DARWIN ELF loader: graph loaded root=%s sources=%zu\n",
                   root_soname, source_count);
    }
    if (!AttachNativeOwner(library.get(), kNativeOwnerGraph, library->graph,
                           &DropRuntimeElfGraph, &error)) {
      SetNativeLoaderError(error_msg, "Rust native owner graph slot failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (!RegisterCachedElfGraph(loader_namespace_id, root_soname, library->graph, &error)) {
      SetNativeLoaderError(error_msg, "Android ELF namespace cache failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (library->fixture_graph &&
        g_elf_fixture_provider_routes.load(std::memory_order_relaxed) !=
            kFixtureAllProviderRouteMask) {
      SetNativeLoaderError(error_msg, "Android ELF did not route all Bionic provider imports");
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (library->fixture_graph) g_elf_fixture_namespace_lifecycle.store(3, std::memory_order_relaxed);
    JNIEnv* art_env = CurrentArtEnv();
    if (art_env == nullptr || art_env->GetJavaVM(&library->art_vm) != JNI_OK ||
        library->art_vm == nullptr) {
      SetNativeLoaderError(error_msg, "Android JNI proxy could not retain ART JavaVM");
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    DarwinArtJniBackend backend{
        library.get(), &ProxyCurrentEnv, &ProxyAttachCurrentThread,
        &ProxyDetachCurrentThread, &ProxyFindClass, &ProxyRegisterNatives,
        &ProxyThrowNew, &ProxyGetMethodId, &ProxyCallMethodV};
    library->proxy = darwin_art_jni_proxy_init(
        library->proxy_storage.data(), library->proxy_storage.size(), &backend);
    if (library->proxy == nullptr ||
        !LookupOptionalElfSymbol(library.get(), "JNI_OnLoad", &library->jni_on_load, &error) ||
        !LookupOptionalElfSymbol(library.get(), "JNI_OnUnload", &library->jni_on_unload, &error) ||
        (library->fixture_graph && (library->jni_on_load == 0 || library->jni_on_unload == 0))) {
      SetNativeLoaderError(error_msg,
                           library->proxy == nullptr ? "Android JNI proxy initialization failed"
                                                     : "Android ELF lifecycle preflight failed: " + error);
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    if (needs_native_bridge == nullptr) {
      SetNativeLoaderError(error_msg, "Android ELF load requires needs_native_bridge ownership");
      TeardownProviderNamespace(library.get());
      return nullptr;
    }
    *needs_native_bridge = true;
    if (library->fixture_graph) {
      g_elf_classified_trampoline_mask.store(0, std::memory_order_relaxed);
      g_elf_fixture_status.store(kElfOpened | kElfBionicProvidersRouted,
                                 std::memory_order_relaxed);
    }
    ElfLibrary* library_value = library.release();
    RuntimeNativeOwner* graph_handle =
        darwin_art_runtime_native_owner_create();
    if (graph_handle == nullptr ||
        darwin_art_runtime_native_owner_attach(
            graph_handle, kNativeOwnerGraphHandle, library_value, nullptr,
            &DropRuntimeElfLibrary) != 0) {
      if (graph_handle != nullptr) {
        (void)darwin_art_runtime_native_owner_destroy(graph_handle);
      }
      (void)DropRuntimeElfLibrary(library_value, nullptr);
      SetNativeLoaderError(error_msg, "Rust graph handle publication failed");
      return nullptr;
    }
    library_value->graph_handle = graph_handle;
    if (std::getenv("DARWIN_ART_DEBUG_GUEST_LIBDL") != nullptr) {
      std::fprintf(stderr,
                   "DARWIN ELF loader: published handle=%p JNI_OnLoad=%p\n",
                   graph_handle, reinterpret_cast<void*>(library_value->jni_on_load));
    }
    BindGuestDsoLoader(library_value);
    return graph_handle;
  }
  if (resolved_path != nullptr && root_is_elf != 0) {
    SetNativeLoaderError(error_msg, "Android ELF discovery failed: " +
                                     ElfError(discovery_status, discovery_error));
    return nullptr;
  }
  // Android linker namespaces do not promote an application DSO into a
  // process-wide symbol pool. A library opened without an application
  // ClassLoader is instead in the boot/agent namespace; JVMTI's agent loader
  // relies on that namespace being visible to RTLD_DEFAULT (the native-method
  // bind contract uses exactly that lookup). Preserve application isolation
  // while matching the platform scope for bootstrap and agent loads.
  const int load_scope = loader == nullptr ? RTLD_GLOBAL : RTLD_LOCAL;
  void* handle = resolved_path == nullptr
                     ? nullptr
                     : dlopen(resolved_path, RTLD_NOW | load_scope);
  if (handle == nullptr && resolved_path != nullptr &&
      NativeBridgeIsSupported(resolved_path)) {
    if (needs_native_bridge == nullptr) {
      SetNativeLoaderError(error_msg,
                           "NativeBridge load requires ownership output");
      return nullptr;
    }
    *needs_native_bridge = true;
    handle = NativeBridgeLoadLibrary(resolved_path, RTLD_NOW);
    if (handle == nullptr) {
      SetNativeLoaderError(error_msg, NativeBridgeGetError());
    }
    return handle;
  }
  if (handle == nullptr && error_msg != nullptr) {
    const char* message = dlerror();
    *error_msg = strdup(message == nullptr ? "Darwin native library load failed" : message);
  }
  return handle;
}

extern "C" bool CloseNativeLibrary(void* handle,
                                    bool needs_native_bridge,
                                    char** error_msg) {
  if (needs_native_bridge) {
    ElfLibrary* library = AsElfLibrary(handle);
    if (library == nullptr) {
      const int status = NativeBridgeUnloadLibrary(handle);
      if (status != 0) {
        SetNativeLoaderError(error_msg, NativeBridgeGetError());
        return false;
      }
      return true;
    }
    DestroyRuntimeElfTrampolines(library);
    const int status = darwin_art_runtime_native_owner_destroy(
        static_cast<RuntimeNativeOwner*>(handle));
    if (status != 0) {
      SetNativeLoaderError(error_msg, "Rust graph owner teardown failed");
      return false;
    }
    return true;
  }
  if (handle == nullptr || dlclose(handle) == 0) return true;
  if (error_msg != nullptr) {
    const char* message = dlerror();
    *error_msg = strdup(message == nullptr ? "Darwin native library close failed" : message);
  }
  return false;
}

}  // namespace android
