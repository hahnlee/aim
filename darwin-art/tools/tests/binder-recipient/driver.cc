#include <dlfcn.h>

#include <cstdio>
#include <unistd.h>

namespace {

using Entry = int (*)(int, const char* const*);
constexpr const char* kEntryName = "darwin_art_binder_recipient_test_entry";

int Fail(const char* operation, const char* detail) {
  std::fprintf(stderr, "%s: %s\n", operation, detail == nullptr ? "unknown error" : detail);
  return 1;
}

}  // namespace

int main(int argc, char* argv[]) {
  // argv[1] is the test dylib; argv[2..6] are the five VM argument paths.
  if (argc != 7) {
    std::fprintf(stderr, "usage: %s testdylib vm-arg1 vm-arg2 vm-arg3 vm-arg4 vm-arg5\n",
                 argc == 0 ? "binder-recipient-driver" : argv[0]);
    return 2;
  }

  void* handle = dlopen(argv[1], RTLD_NOW | RTLD_GLOBAL);
  if (handle == nullptr) return Fail("dlopen", dlerror());

  dlerror();
  void* address = dlsym(handle, kEntryName);
  const char* symbol_error = dlerror();
  if (symbol_error != nullptr || address == nullptr) {
    return Fail("dlsym", symbol_error);
  }

  Entry entry = reinterpret_cast<Entry>(address);
  // ART startup is not safely unloadable. The process owns this handle until
  // exit; deliberately do not call dlclose after the test entry returns.
  const int status = entry(argc - 2, argv + 2);
  std::fflush(nullptr);
  _exit(status);
}
