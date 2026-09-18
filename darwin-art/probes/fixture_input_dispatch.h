#pragma once

#include "fixture_input_channel.h"
#include "fixture_input_exchange.h"

namespace darwin_art_graphics_fixture {
struct GraphicsFixtureState;
struct FixtureInputDispatchResult;

struct FixtureInputEndpoint final {
  FixtureInputChannel channel;
  FixtureInputExchange exchange;
  bool retired = false; // Owner-thread publication precedes Java disposal.
  bool dispatch_admitted = false; // Covers owner-Looper progress, not only I/O.
};

using FixtureOwnerProgress = bool (*)(void*, JNIEnv*);

// TEST ONLY: public guest Java channel APIs + shared wire ABI. The callback
// progresses the genuine fixture owner Looper; receiver internals stay hidden.
FixtureInputDispatchResult DispatchFixtureInputPacket(
    GraphicsFixtureState*, JNIEnv*, jobject view_root,
    const darwin_art::DarwinArtInputPacket&, FixtureOwnerProgress, void*);
void DisposeFixtureInputEndpoints(GraphicsFixtureState*, JNIEnv*);
}  // namespace darwin_art_graphics_fixture
