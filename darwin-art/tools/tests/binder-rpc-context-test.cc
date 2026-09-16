#include "binder/rpc_context.h"
#include "binder/rpc_identity.h"

#include <binder/Binder.h>
#include <binder/Parcel.h>
#include <binder/RpcServer.h>
#include <binder/RpcSession.h>

#include <atomic>
#include <cassert>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <thread>
#include <unistd.h>

namespace {

constexpr int32_t kTestAndroidUid = 10123;

class Root final : public android::BBinder {
 public:
  android::status_t onTransact(uint32_t code, const android::Parcel& data,
                               android::Parcel* reply, uint32_t) override {
    int32_t calling_pid = -1;
    int32_t calling_uid = -1;
    if (!darwin_art_binder_rpc_identity_current(&calling_pid, &calling_uid,
                                                 nullptr)) {
      return android::PERMISSION_DENIED;
    }
    observed_pid.store(calling_pid, std::memory_order_release);
    observed_uid.store(calling_uid, std::memory_order_release);
    if (code == android::IBinder::FIRST_CALL_TRANSACTION) {
      return reply->writeInt32(data.readInt32() + 1);
    }
    if (code == android::IBinder::FIRST_CALL_TRANSACTION + 1) {
      std::this_thread::sleep_for(std::chrono::milliseconds(250));
      completed.store(true, std::memory_order_release);
      return android::OK;
    }
    return android::UNKNOWN_TRANSACTION;
  }

  std::atomic<bool> completed{false};
  std::atomic<int32_t> observed_pid{-1};
  std::atomic<int32_t> observed_uid{-1};
};

}  // namespace

extern "C" int32_t darwin_art_runtime_registered_process_uid(uint32_t pid) {
  return pid == static_cast<uint32_t>(getpid()) ? kTestAndroidUid : -1;
}

int main() {
  alarm(20);
  char directory[] = "/tmp/darwin-art-binder-rpc.XXXXXX";
  assert(mkdtemp(directory) != nullptr);
  const std::string path = std::string(directory) + "/context.sock";

  android::sp<Root> local = android::sp<Root>::make();
  android::sp<android::RpcServer> server = android::RpcServer::make();
  server->setMaxThreads(2);
  server->setSupportedFileDescriptorTransportModes(
      {android::RpcSession::FileDescriptorTransportMode::UNIX});
  server->setRootObject(local);
  assert(server->setupUnixDomainServer(path.c_str()) == android::OK);
  server->start();

  android::sp<android::IBinder> remote =
      darwin_art::binder::ConnectRpcContext(path.c_str());
  assert(remote != nullptr && remote->remoteBinder() != nullptr);
  android::Parcel data;
  android::Parcel reply;
  data.markForBinder(remote);
  assert(data.writeInt32(41) == android::OK);
  const android::status_t sync_status = remote->transact(
      android::IBinder::FIRST_CALL_TRANSACTION, data, &reply, 0);
  if (sync_status != android::OK) {
    std::fprintf(stderr, "Binder RPC sync transaction failed: %d\n",
                 sync_status);
  }
  assert(sync_status == android::OK);
  assert(reply.readInt32() == 42);
  assert(local->observed_pid.load(std::memory_order_acquire) == getpid());
  assert(local->observed_uid.load(std::memory_order_acquire) ==
         kTestAndroidUid);

  const auto start = std::chrono::steady_clock::now();
  android::Parcel oneway;
  oneway.markForBinder(remote);
  assert(remote->transact(android::IBinder::FIRST_CALL_TRANSACTION + 1,
                          oneway, nullptr,
                          android::IBinder::FLAG_ONEWAY) == android::OK);
  const auto elapsed = std::chrono::steady_clock::now() - start;
  assert(elapsed < std::chrono::milliseconds(100));
  while (!local->completed.load(std::memory_order_acquire)) {
    std::this_thread::yield();
  }
  assert(local->observed_pid.load(std::memory_order_acquire) == 0);
  assert(local->observed_uid.load(std::memory_order_acquire) ==
         kTestAndroidUid);

  remote.clear();
  assert(server->shutdown());
  server.clear();
  local.clear();
  unlink(path.c_str());
  rmdir(directory);
  std::puts("original Binder RPC context: authenticated sync/one-way identity PASS");
}
