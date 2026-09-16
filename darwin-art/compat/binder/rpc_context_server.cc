#include "rpc_context_server.h"

#include "../process/service_endpoint.h"

#include <binder/RpcServer.h>
#include <binder/RpcSession.h>
#include <binder/unique_fd.h>

#include "android_util_Binder.h"

#include <cstdio>
#include <unistd.h>

namespace darwin_art::binder {

int ServeRpcContext(JNIEnv* env, jobject root, const char* path) {
  if (env == nullptr || root == nullptr || path == nullptr || *path == '\0') {
    return 70;
  }
  android::sp<android::IBinder> binder =
      android::ibinderForJavaObject(env, root);
  if (binder == nullptr || env->ExceptionCheck()) return 70;

  process::ServiceEndpoint listener(path);
  if (!listener) return 70;
  const int server_fd = dup(listener.descriptor());
  if (server_fd < 0) return 70;

  android::sp<android::RpcServer> server = android::RpcServer::make();
  if (server == nullptr) {
    close(server_fd);
    return 70;
  }
  server->setMaxThreads(8);
  server->setSupportedFileDescriptorTransportModes(
      {android::RpcSession::FileDescriptorTransportMode::UNIX});
  server->setRootObject(binder);
  if (server->setupRawSocketServer(android::binder::unique_fd(server_fd)) !=
      android::OK) {
    return 70;
  }
  if (listener.publish_ready(process::ServiceReadiness::Binder) ==
      process::ReadinessPublication::Failed) {
    return 70;
  }
  std::fprintf(stderr, "ART Binder RPC: service endpoint listening socket=%s\n",
               path);
  server->join();
  listener.report_lost(process::ServiceReadiness::Binder);
  return 70;
}

}  // namespace darwin_art::binder
