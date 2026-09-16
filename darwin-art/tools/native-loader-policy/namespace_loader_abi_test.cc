#include "loader/namespace_loader_abi.h"
#include "loader/namespace_handles.h"
#include <nativeloader/dlext_namespaces.h>
#include <cassert>
#include <cstdio>
#include <cstring>
#include <vector>
#include <csignal>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>
extern "C" void __loader_android_get_LD_LIBRARY_PATH(char*, size_t);
extern "C" int darwin_art_linker_dlclose(void*);

void TestNamespaceLoaderAbi(darwin_art::loader::NamespaceHandles& owner) {
  std::string paths;
  assert(owner.DefaultLibraryPath(&paths) && !paths.empty());
  std::vector<char> buffer(paths.size() + 1, '!');
  __loader_android_get_LD_LIBRARY_PATH(buffer.data(), buffer.size());
  assert(paths == buffer.data());
  const pid_t child_pid = fork();
  assert(child_pid >= 0);
  if (child_pid == 0) {
    const rlimit no_core{0, 0};
    if (setrlimit(RLIMIT_CORE, &no_core) != 0) _exit(90);
    __loader_android_get_LD_LIBRARY_PATH(buffer.data(), paths.size());
    _exit(91);
  }
  int status = 0;
  assert(waitpid(child_pid, &status, 0) == child_pid);
  assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
  std::puts("Bionic default path query: configured paths, exact capacity, short buffer fatal PASS");
  assert(__loader_android_get_exported_namespace("default") == owner.Exported("default"));
  assert(!__loader_android_get_exported_namespace("absent-namespace"));
  auto* child = __loader_android_create_namespace("libdl-abi-child", nullptr, nullptr,
      ANDROID_NAMESPACE_TYPE_ISOLATED, nullptr, owner.Anonymous(), nullptr);
  assert(child);
  assert(__loader_android_link_namespaces(child, owner.Exported("default"), "libc.so"));
  assert(!__loader_android_create_namespace("invalid-type", nullptr, nullptr,
      UINT64_MAX, nullptr, owner.Anonymous(), nullptr));
  assert(darwin_art_linker_dlerror());
  // Exercise the actual Bionic forwarding ABI against the installed registry.
  // These are Android opaque handles, never dyld handles.
  void* opened = __loader_dlopen("libc.so", 2, nullptr);
  assert(opened && darwin_art_linker_dlclose(opened) == 0);
  android_dlextinfo extension{};
  extension.flags = ANDROID_DLEXT_USE_NAMESPACE;
  extension.library_namespace = child;
  opened = __loader_android_dlopen_ext("libc.so", 2, &extension, nullptr);
  assert(opened && darwin_art_linker_dlclose(opened) == 0);
  // The isolated child links only libc, not every library in the default NS.
  assert(!__loader_android_dlopen_ext("libm.so", 2, &extension, nullptr));
  assert(darwin_art_linker_dlerror());
  extension.flags |= uint64_t{1} << 63;
  assert(!__loader_android_dlopen_ext("libc.so", 2, &extension, nullptr));
  assert(darwin_art_linker_dlerror());
  std::puts("Bionic open forwarding: Android handles, namespace isolation, unsupported flags PASS");
  std::puts("Bionic loader namespace ABI: owner identity, creation/link, invalid flags PASS");
}
