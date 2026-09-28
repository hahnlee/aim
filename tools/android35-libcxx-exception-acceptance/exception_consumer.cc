#include <stdexcept>
#include <string>

namespace {

int g_cleanup_count = 0;

struct Cleanup {
  std::string storage = "unwind-cleanup-crosses-a-nontrivial-frame";

  ~Cleanup() {
    if (!storage.empty()) {
      ++g_cleanup_count;
    }
  }
};

__attribute__((noinline)) void ThrowAcrossCleanup() {
  Cleanup cleanup;
  throw std::runtime_error("accepted");
}

}  // namespace

extern "C" __attribute__((visibility("default"))) int
aim_libcxx_exception() {
  g_cleanup_count = 0;
  try {
    ThrowAcrossCleanup();
  } catch (const std::runtime_error& error) {
    return g_cleanup_count == 1 && std::string(error.what()) == "accepted"
               ? 73
               : -1;
  }
  return -2;
}

extern "C" __attribute__((visibility("default"))) int JNI_OnLoad(void*, void*) {
  return aim_libcxx_exception() == 73 ? 0x00010006 : -1;
}
