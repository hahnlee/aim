#pragma once

#include "endpoint_lifetime.h"
#include "darwin_art/darwin_art.h"
#include <jni.h>
#include <memory>

namespace darwin_art::binder {

// The embedding process supplies the terminal metadata of its exact single
// authenticated kernel Binder endpoint. No FD lookup creates authority here.
bool InstallNativeBinderAuthority(const darwin_art_binder_authority_hooks_t* hooks) noexcept;
std::shared_ptr<EndpointLifetime> CaptureNativeBinderEndpoint(JNIEnv* env,
                                                           jobject capability) noexcept;
// Native factory, obituary pins and Rust release tails remain counted after
// admission closes. Binder worker/ART quiescence is a separate shutdown gate.
bool CloseNativeBinderEndpointAdmission() noexcept;
bool PollNativeBinderEndpointsQuiesced() noexcept;
// A started AOSP pool has no joined-worker proof in this runtime yet. Reusable
// teardown must fail closed; Android application OS process exit is separate.
bool NativeBinderWorkersQuiesced() noexcept;

}  // namespace darwin_art::binder
