#include "compat/darwin_surface_internal.h"
#include <cassert>
#include <cstdio>
#include <memory>
#include <cstring>
#include <limits>
#include <objc/runtime.h>
#include <vector>

namespace {
NSEvent* Mouse(NSEventType type) {
  return [NSEvent mouseEventWithType:type location:NSMakePoint(10, 20)
      modifierFlags:0 timestamp:0 windowNumber:0 context:nil eventNumber:1
      clickCount:1 pressure:1];
}
NSEvent* Key(NSEventType type, bool repeat = false) {
  return [NSEvent keyEventWithType:type location:NSZeroPoint modifierFlags:0
      timestamp:0 windowNumber:0 context:nil characters:@"a"
      charactersIgnoringModifiers:@"a" isARepeat:repeat keyCode:0];
}

enum class Reentry { None, CancelDown, DownOnUp, DownOnCancel, DetachOnCancel, CancelOnCancel,
                     KeyDownOnUp, DestroyDown };
struct Fixture {
  struct SurfaceDeleter {
    void operator()(AimSurface* value) const {
      assert(aim_surface_destroy(value) == AIM_SURFACE_OK);
    }
  };
  std::unique_ptr<AimSurface, SurfaceDeleter> surface;
  std::vector<AimPointerEventV2> pointers;
  std::vector<AimKeyEventV1> keys;
  Reentry reentry = Reentry::None;
  int binding_pins = 0;

  Fixture() {
    const AimSurfaceCreateInfo info{360, 640, "Content view test", false, false};
    AimSurfaceResult result = AIM_SURFACE_INVALID_ARGUMENT;
    surface.reset(aim_surface_create(&info, &result));
    assert(result == AIM_SURFACE_OK && surface != nullptr);
    assert(surface->window == nil && surface->view != nil);
    const AimSurfaceInputSink sink{
        .version = 1, .size = sizeof(AimSurfaceInputSink), .context = this,
        .retain_context = Retain, .release_context = Release,
        .pointer = Pointer, .key = Keyboard};
    assert(aim_surface_set_input_sink(surface.get(), &sink) == AIM_SURFACE_OK);
  }
  ~Fixture() {
    surface.reset(); // callbacks settle while fixture packet storage is live
    assert(binding_pins == 0);
  }
  static void Retain(void* context) {
    ++static_cast<Fixture*>(context)->binding_pins;
  }
  static void Release(void* context) {
    auto& pins = static_cast<Fixture*>(context)->binding_pins;
    assert(pins > 0);
    --pins;
  }

  static AimSurfaceInputResult Pointer(void* context,
                                           const AimPointerEventV2* event) {
    auto& fixture = *static_cast<Fixture*>(context);
    fixture.pointers.push_back(*event);
    const auto action = fixture.reentry;
    if ((action == Reentry::CancelDown || action == Reentry::DestroyDown) &&
        event->action == AIM_POINTER_DOWN) {
      fixture.reentry = Reentry::None;
      if (action == Reentry::DestroyDown) fixture.surface.reset();
      else [fixture.surface->view cancelPointerStream];
    } else if (action == Reentry::DownOnUp && event->action == AIM_POINTER_UP) {
      fixture.reentry = Reentry::None;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
    } else if (action == Reentry::DownOnCancel &&
               event->action == AIM_POINTER_CANCEL) {
      fixture.reentry = Reentry::None;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
    } else if (action == Reentry::DetachOnCancel &&
               event->action == AIM_POINTER_CANCEL) {
      fixture.reentry = Reentry::None;
      [fixture.surface->view setOwnerSurface:nullptr];
    } else if (action == Reentry::CancelOnCancel &&
               event->action == AIM_POINTER_CANCEL) {
      fixture.reentry = Reentry::None;
      [fixture.surface->view cancelPointerStream];
    }
    return AIM_SURFACE_INPUT_QUEUED;
  }

  static AimSurfaceInputResult Keyboard(void* context,
                                             const AimKeyEventV1* event) {
    auto& fixture = *static_cast<Fixture*>(context);
    fixture.keys.push_back(*event);
    if (fixture.reentry == Reentry::KeyDownOnUp && event->action == 1) {
      fixture.reentry = Reentry::None;
      [fixture.surface->view keyDown:Key(NSEventTypeKeyDown)];
    }
    return AIM_SURFACE_INPUT_QUEUED;
  }
};
}  // namespace

int main() {
  @autoreleasepool {
    assert([NSThread isMainThread]);
    {
      Fixture fixture;
      fixture.reentry = Reentry::CancelDown;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      assert(fixture.pointers.size() == 2);
      assert(fixture.pointers.back().action == AIM_POINTER_CANCEL);
      [fixture.surface->view cancelPointerStream];
      assert(fixture.pointers.size() == 2); // no callback-tail resurrection
    }
    {
      Fixture fixture;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      fixture.reentry = Reentry::DownOnUp;
      [fixture.surface->view mouseUp:Mouse(NSEventTypeLeftMouseUp)];
      assert(fixture.pointers.size() == 3);
      [fixture.surface->view cancelPointerStream];
      assert(fixture.pointers.size() == 4);
      assert(fixture.pointers.back().action == AIM_POINTER_CANCEL);
    }
    {
      Fixture fixture;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      fixture.reentry = Reentry::DownOnCancel;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      assert(fixture.pointers.size() == 3); // outer stale DOWN is abandoned
      [fixture.surface->view cancelPointerStream];
      assert(fixture.pointers.size() == 4);
    }
    {
      Fixture fixture;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      fixture.reentry = Reentry::DetachOnCancel;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      assert(fixture.pointers.size() == 2);
      assert(fixture.surface->view.ownerSurface == nullptr);
    }
    {
      Fixture fixture;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      fixture.reentry = Reentry::CancelOnCancel;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      assert(fixture.pointers.size() == 2); // nested inactive cancel revokes outer DOWN
      [fixture.surface->view cancelPointerStream];
      assert(fixture.pointers.size() == 2);
    }
    {
      Fixture fixture;
      [fixture.surface->view keyDown:Key(NSEventTypeKeyDown)];
      fixture.reentry = Reentry::KeyDownOnUp;
      [fixture.surface->view keyUp:Key(NSEventTypeKeyUp)];
      [fixture.surface->view keyDown:Key(NSEventTypeKeyDown, true)];
      assert(fixture.keys.size() == 4);
      assert(fixture.keys[3].down_time_nanos == fixture.keys[2].down_time_nanos);
      assert(fixture.keys[3].repeat_count == 1);
    }
    {
      Fixture fixture;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      // Arithmetic-boundary injection is test-only; no fixture API is exported.
      Ivar revision = class_getInstanceVariable(NSClassFromString(@"AimMetalView"), "_inputRevision");
      assert(revision != nullptr);
      const uint64_t maximum = std::numeric_limits<uint64_t>::max();
      auto* storage = static_cast<char*>((__bridge void*)fixture.surface->view);
      std::memcpy(storage + ivar_getOffset(revision), &maximum, sizeof(maximum));
      [fixture.surface->view mouseDragged:Mouse(NSEventTypeLeftMouseDragged)];
      assert(fixture.pointers.size() == 2);
      assert(fixture.pointers.back().action == AIM_POINTER_CANCEL);
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      assert(fixture.pointers.size() == 2);
      uint64_t observed = 0;
      std::memcpy(&observed, storage + ivar_getOffset(revision), sizeof(observed));
      assert(observed == maximum);
    }
    {
      Fixture fixture;
      fixture.reentry = Reentry::DestroyDown;
      [fixture.surface->view mouseDown:Mouse(NSEventTypeLeftMouseDown)];
      // Offscreen destroy has no NSWindow willClose callback. This proves
      // callback-tail view retention, not pointer-cancellation settlement.
      assert(fixture.surface == nullptr && fixture.pointers.size() == 1);
    }
  }
  std::puts("Product AppKit content view: callback reentry/replacement/destruction PASS");
}
