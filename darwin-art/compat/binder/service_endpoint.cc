#include "service_endpoint.h"
#include "../darwin_binder_wire.h"
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
#include "rpc_context_server.h"
#endif
#include "../process/service_endpoint.h"
#include <cerrno>
#include <cstdio>
#include <sys/socket.h>
#include <unistd.h>

namespace darwin_art {
int ServeBinderServiceEndpoint(JNIEnv* env, jobject registry, const char* path) {
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  return binder::ServeRpcContext(env, registry, path);
#else
  if (env == nullptr || registry == nullptr || path == nullptr) return 70;
  process::ServiceEndpoint listener(path);
  if (!listener) return 70;
  if (listener.publish_ready(process::ServiceReadiness::Binder) ==
      process::ReadinessPublication::Failed) return 70;
  std::fprintf(stderr, "ART Binder: service endpoint listening socket=%s\n", path);
  for (;;) {
    const int client = accept(listener.descriptor(), nullptr, nullptr);
    if (client < 0) {
      if (errno == EINTR) continue;
      listener.report_lost(process::ServiceReadiness::Binder);
      return 70;
    }
    if (!StartServingRemoteBinder(env, client, registry)) close(client);
  }
#endif
}
}
