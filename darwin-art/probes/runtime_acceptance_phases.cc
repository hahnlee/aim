#include "runtime_acceptance_phases.h"
#include "runtime_acceptance_state.h"

#include <iostream>

#include "runtime_network_probe.h"

namespace darwin_art_network_phase {

int run(JNIEnv* env, const char* fixture_path, jobject app_loader,
        jclass fixture_class) {
  if (darwin_art_network_load_fixture(env, fixture_path, app_loader,
                                      fixture_class) != 0) {
    return 47;
  }
  darwin_art_acceptance::record_network_elf_loaded();
  BoundedLoopbackHttpServer server;
  if (!server.Start()) {
    std::cerr << "ART Android network loopback listener failed\n";
    return 47;
  }
  jmethodID loopback = env->GetStaticMethodID(
      fixture_class, "nativeLoopbackHttp", "(I)I");
  const jint network_result =
      loopback == nullptr
          ? -1
          : env->CallStaticIntMethod(fixture_class, loopback,
                                     static_cast<jint>(server.port()));
  const bool server_ok = server.Stop();
  if (network_result != 42 || !server_ok || env->ExceptionCheck()) {
    std::cerr << "ART Android network loopback failed, result="
              << network_result << " server=" << server_ok << "\n";
    return 47;
  }
  std::cout << "ART Android network: JavaVMExt+JNI_OnLoad loopback-HTTP=42 "
               "socket+DNS=closed Internet=no\n"
            << std::flush;
  return 0;
}

}  // namespace darwin_art_network_phase
