// Exercise original NativeLoader boot/APEX selection against real Android ELF.
// This is an integration diagnostic, not an ART or APK acceptance gate.
#include <nativeloader/native_loader.h>
#include <cstdio>

extern "C" void* darwin_art_linker_dlsym(void*, const char*);
extern "C" char* darwin_art_linker_dlerror();

int AuditBootLibraries() {
  struct Entry { const char* library; const char* caller; const char* symbol; };
  const Entry entries[] = {
      {"libicu.so", "/apex/com.android.i18n/javalib/core-icu4j.jar", "u_getVersion"},
      {"libjavacrypto.so", "/apex/com.android.conscrypt/javalib/conscrypt.jar", "JNI_OnLoad"},
  };
  int failures = 0;
  for (const auto& entry : entries) {
    bool bridge = false;
    char* error = nullptr;
    void* handle = android::OpenNativeLibrary(nullptr, 36, entry.library, nullptr,
        entry.caller, nullptr, &bridge, &error);
    if (!handle) {
      std::fprintf(stderr, "BOOT LOAD FAIL %s caller=%s: %s\n", entry.library,
          entry.caller, error ? error : "no loader error");
      android::NativeLoaderFreeErrorMessage(error);
      ++failures;
      continue;
    }
    android::NativeLoaderFreeErrorMessage(error);
    if (bridge || !darwin_art_linker_dlsym(handle, entry.symbol)) {
      const char* symbol_error = bridge ? "unexpected native bridge" : darwin_art_linker_dlerror();
      std::fprintf(stderr, "BOOT SYMBOL FAIL %s symbol=%s bridge=%d error=%s\n",
          entry.library, entry.symbol, bridge,
          symbol_error ? symbol_error : "no loader error");
      ++failures;
    } else {
      std::fprintf(stderr, "BOOT LOAD PASS %s caller=%s symbol=%s\n",
          entry.library, entry.caller, entry.symbol);
    }
    error = nullptr;
    if (!android::CloseNativeLibrary(handle, bridge, &error)) {
      std::fprintf(stderr, "BOOT CLOSE FAIL %s: %s\n", entry.library,
          error ? error : "no loader error");
      ++failures;
    }
    android::NativeLoaderFreeErrorMessage(error);
  }
  return failures ? 1 : 0;
}
