#include "loader/library_warning.h"
#include <cassert>
#include <cstdio>
#include <string>
#include <thread>
extern "C" void darwin_art_linker_set_error(const char*);
extern "C" char* darwin_art_linker_dlerror();

namespace {
struct Result { bool empty = false; std::string text; };
void Capture(void* opaque, const char* text) {
  auto& result = *static_cast<Result*>(opaque);
  result.empty = !text;
  result.text = text ? text : "";
}
void Reenter(void* opaque, const char* text) {
  Capture(opaque, text);
  Result nested;
  __loader_android_dlwarning(&nested, Capture);
  assert(nested.empty); // Original queue drains before callback.
  darwin_art::loader::AppendLinkerWarning("next.so", "new warning");
}
}
void TestLibraryWarnings() {
  Result result;
  __loader_android_dlwarning(&result, Capture);
  assert(result.empty);
  std::thread producer([] {
    darwin_art::loader::AppendLinkerWarning("/system/lib64/one.so", "missing", "value");
    darwin_art::loader::AppendLinkerWarning("two.so", "second");
  });
  producer.join();
  darwin_art_linker_set_error("independent loader error");
  __loader_android_dlwarning(&result, Reenter);
  assert(result.text == "one.so: missing \"value\"\ntwo.so: second");
  const char* error = darwin_art_linker_dlerror();
  assert(error && std::string(error) == "independent loader error");
  __loader_android_dlwarning(&result, Capture);
  assert(result.text == "next.so: new warning");
  __loader_android_dlwarning(&result, Capture);
  assert(result.empty);
  std::puts("AOSP dlwarning: process-wide queue, formatting, consume-before-callback, reentry PASS");
}
