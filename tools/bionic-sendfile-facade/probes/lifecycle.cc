#include "aim_bionic_sendfile.h"

#include <atomic>
#include <thread>

namespace {
std::atomic<bool> entered{false};
std::atomic<bool> release_transfer{false};

AimBionicSendfileTransferStatus Block(
    void*, const AimBionicSendfileRequest* request,
    AimBionicSendfileResult* result) {
  entered.store(true, std::memory_order_release);
  while (!release_transfer.load(std::memory_order_acquire)) std::this_thread::yield();
  *result = {AIM_BIONIC_SENDFILE_ABI_VERSION, 0, 0, request->offset};
  return AIM_BIONIC_SENDFILE_TRANSFER_OK;
}
}

int main() {
  if (aim_bionic_sendfile_activate(&Block, nullptr) !=
      AIM_BIONIC_SENDFILE_LIFECYCLE_OK) return 1;
  std::thread caller([] { if (aim_bionic_sendfile(2, 1, nullptr, 1) != 0) __builtin_trap(); });
  while (!entered.load(std::memory_order_acquire)) std::this_thread::yield();
  std::atomic<bool> drained{false};
  std::thread drain([&] {
    if (aim_bionic_sendfile_deactivate() !=
        AIM_BIONIC_SENDFILE_LIFECYCLE_OK) __builtin_trap();
    drained.store(true, std::memory_order_release);
  });
  for (int spin = 0; spin != 1000; ++spin) std::this_thread::yield();
  if (drained.load(std::memory_order_acquire)) return 2;
  release_transfer.store(true, std::memory_order_release);
  caller.join();
  drain.join();
  return drained.load(std::memory_order_acquire) ? 0 : 3;
}
