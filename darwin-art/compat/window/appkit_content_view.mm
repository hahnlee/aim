#import "appkit_content_view.h"
#include "../darwin_surface_internal.h"
#include "../darwin_android_time.h"
#include "../input/darwin_hardware_key_translation.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <limits>
#include <mutex>
#include <unordered_map>

namespace {

std::shared_ptr<DarwinArtSurfaceInputSinkBinding> SnapshotInputSink(
    DarwinArtSurface* surface) {
  if (surface == nullptr) return nullptr;
  std::lock_guard<std::mutex> lock(surface->input_sink_mutex);
  return surface->input_sink;
}

uint64_t AndroidEventTimeNanos() {
  return static_cast<uint64_t>(darwin_art::AndroidUptimeNanos());
}

}  // namespace

@implementation DarwinArtMetalView {
  CAMetalLayer* _metalLayer;
  DarwinArtSurface* _ownerSurface;
  std::unordered_map<unsigned short, uint64_t> _keyDownTimes;
  std::unordered_map<unsigned short, uint32_t> _keyRepeatCounts;
  BOOL _pointerActive;
  uint64_t _nextPointerSequence;
  uint64_t _nextKeySequence;
  uint64_t _downTimeNanos;
  uint64_t _inputRevision;
  BOOL _inputAdmissionClosed;
}

- (instancetype)initWithFrame:(NSRect)frame
                       device:(id<MTLDevice>)device
                    pixelSize:(CGSize)pixelSize
                 contentScale:(CGFloat)contentScale {
  self = [super initWithFrame:frame];
  if (self != nil) {
    self.wantsLayer = YES;
    _metalLayer = [CAMetalLayer layer];
    _metalLayer.device = device;
    _metalLayer.pixelFormat = MTLPixelFormatBGRA8Unorm;
    // Android's window and child SurfaceControl buffers are authored in the
    // sRGB display space.  AppKit otherwise leaves CAMetalLayer's color
    // space unspecified, allowing the window server to apply a different
    // display profile from the IOSurface/Skia path.  That made Chrome's
    // toolbar surfaces acquire subtly different red/blue balance.  Keep the
    // presentation target explicitly sRGB so every Android surface follows
    // one color transform through scanout.
    CGColorSpaceRef srgb = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    _metalLayer.colorspace = srgb;
    if (srgb != nullptr) CGColorSpaceRelease(srgb);
    // The current presenter uses the CAMetalLayer drawable as a blit
    // destination. Metal forbids blits into framebuffer-only textures.
    _metalLayer.framebufferOnly = NO;
    _metalLayer.contentsScale = contentScale;
    _metalLayer.drawableSize = pixelSize;
    self.layer = _metalLayer;
    _pointerActive = NO;
    _ownerSurface = nullptr;
    _nextPointerSequence = 1;
    _nextKeySequence = 1;
    _downTimeNanos = 0;
  }
  return self;
}

- (void)setOwnerSurface:(DarwinArtSurface*)surface {
  // A delayed/reentrant operation cannot continue against a replacement
  // binding, including replacement with the same numeric surface address.
  if (_inputRevision == std::numeric_limits<uint64_t>::max()) {
    _inputAdmissionClosed = YES;
  } else {
    ++_inputRevision;
  }
  _ownerSurface = surface;
}

- (DarwinArtSurface*)ownerSurface {
  return _ownerSurface;
}

- (void)signalOwnerWake {
  DarwinArtSurface* surface = _ownerSurface;
  if (surface == nullptr) return;
  DarwinArtSurfaceOwnerWakeCallback callback = nullptr;
  void* context = nullptr;
  {
    std::lock_guard<std::mutex> lock(surface->owner_wake_mutex);
    callback = surface->owner_wake_callback;
    context = surface->owner_wake_context;
  }
  if (callback != nullptr) {
    callback(context);
  }
}

- (BOOL)isFlipped {
  return YES;
}

- (CAMetalLayer*)metalLayer {
  return _metalLayer;
}

- (void)updateDrawableSize {
  if (_metalLayer == nil) return;
  const CGFloat scale = _metalLayer.contentsScale > 0.0
                            ? _metalLayer.contentsScale
                            : 1.0;
  const NSRect bounds = self.bounds;
  _metalLayer.drawableSize = CGSizeMake(
      std::max<CGFloat>(1.0, std::ceil(bounds.size.width * scale)),
      std::max<CGFloat>(1.0, std::ceil(bounds.size.height * scale)));
}

- (BOOL)acceptsFirstResponder {
  return YES;
}

- (void)enqueuePointerEvent:(NSEvent*)event
                     action:(DarwinArtPointerAction)action {
  __attribute__((objc_precise_lifetime)) DarwinArtMetalView* kept_view = self;
  if (_inputAdmissionClosed ||
      _inputRevision == std::numeric_limits<uint64_t>::max()) {
    _inputAdmissionClosed = YES;
    [kept_view cancelPointerStream];
    return;
  }
  const uint64_t revision = ++_inputRevision;
  NSPoint point = [self convertPoint:event.locationInWindow fromView:nil];
  const NSRect bounds = self.bounds;
  // Keep the stream alive while a button is held even after the cursor leaves
  // the view. Android receives those coordinates and decides whether the
  // gesture remains owned by the child; dropping them here makes an eventual
  // mouseUp indistinguishable from a lost pointer. A new DOWN while the old
  // stream is still active is repaired with an explicit CANCEL.
  if (action == DARWIN_ART_POINTER_DOWN && _pointerActive) {
    [kept_view cancelPointerStream];
    // CANCEL may detach this owner or recursively admit a newer stream.
    if (_inputAdmissionClosed || revision == std::numeric_limits<uint64_t>::max() ||
        _inputRevision != revision + 1) return;
  }
  // NSEvent coordinates are AppKit points, while Android's retained view is
  // laid out in the CAMetalLayer drawable's backing pixels.  Derive the
  // mapping from the live layer instead of assuming the launcher's requested
  // scale is still the presentation scale after a Retina/resize transition.
  const CGSize drawable_size = _metalLayer.drawableSize;
  const auto backing = _ownerSurface == nullptr ? nullptr
      : _ownerSurface->backing_owner.Acquire();
  const CGFloat android_width =
      backing != nullptr && backing->logical_width() != 0
          ? backing->logical_width()
          : drawable_size.width;
  const CGFloat android_height =
      backing != nullptr && backing->logical_height() != 0
          ? backing->logical_height()
          : drawable_size.height;
  const CGFloat x_scale = bounds.size.width > 0.0
                              ? android_width / bounds.size.width
                              : 1.0;
  const CGFloat y_scale = bounds.size.height > 0.0
                              ? android_height / bounds.size.height
                              : 1.0;
  // DarwinArtMetalView is flipped, so convertPoint already returns a
  // top-left-origin Y coordinate. Flipping it a second time made a click near
  // the top of the window arrive near the bottom of Android/Blink (for
  // example input-field y=188 became body y=888 on a 1280 px surface).
  const CGFloat android_y = point.y;
  const uint64_t event_time_nanos = AndroidEventTimeNanos();
  if (action == DARWIN_ART_POINTER_DOWN) _downTimeNanos = event_time_nanos;
  const DarwinArtPointerEventV2 packet{
      .version = 2,
      .size = static_cast<uint32_t>(sizeof(DarwinArtPointerEventV2)),
      .action = static_cast<uint32_t>(action),
      // Android app players conventionally translate the primary host click
      // into a touchscreen stream. A distinct mouse packet remains available
      // in ABI v2 for explicit external-mouse integrations.
      .flags = 0,
      .sequence = _nextPointerSequence++,
      .event_time_nanos = event_time_nanos,
      .down_time_nanos = _downTimeNanos,
      .pointer_id = 0,
      .pointer_count = 1,
      .x = static_cast<float>(point.x * x_scale),
      .y = static_cast<float>(android_y * y_scale),
      .raw_x = static_cast<float>(point.x * x_scale),
      .raw_y = static_cast<float>(android_y * y_scale),
      .pressure = 1.0f,
      .size_value = 1.0f,
  };
  const auto sink = SnapshotInputSink(_ownerSurface);
  // Commit this packet before any external sink/wake callback can reenter.
  // No callback-tail mutation may overwrite a replacement input stream.
  if (action == DARWIN_ART_POINTER_DOWN) {
    _pointerActive = YES;
  } else if (action == DARWIN_ART_POINTER_UP ||
             action == DARWIN_ART_POINTER_CANCEL) {
    _pointerActive = NO;
    _downTimeNanos = 0;
  }
  const DarwinArtSurfaceInputResult input_result =
      sink == nullptr
          ? DARWIN_ART_SURFACE_INPUT_NO_SINK
          : sink->sink.pointer(sink->sink.context, &packet);
  if (input_result != DARWIN_ART_SURFACE_INPUT_QUEUED &&
      std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::fprintf(stderr,
                 "ART Android input sink pointer status=%u action=%u "
                 "sequence=%llu\n",
                 static_cast<unsigned int>(input_result), packet.action,
                 static_cast<unsigned long long>(packet.sequence));
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::fprintf(stderr,
                 "ART AppKit pointer action=%u sequence=%llu point=%g,%g "
                 "android=%g,%g enqueue=%u\n",
                 static_cast<unsigned int>(packet.action),
                 static_cast<unsigned long long>(packet.sequence), point.x,
                 point.y, static_cast<double>(packet.x),
                 static_cast<double>(packet.y),
                 static_cast<unsigned int>(input_result));
  }
  [kept_view signalOwnerWake];
}

- (void)mouseDown:(NSEvent*)event {
  [self enqueuePointerEvent:event action:DARWIN_ART_POINTER_DOWN];
}

- (void)mouseUp:(NSEvent*)event {
  [self enqueuePointerEvent:event action:DARWIN_ART_POINTER_UP];
}

- (void)mouseDragged:(NSEvent*)event {
  [self enqueuePointerEvent:event action:DARWIN_ART_POINTER_MOVE];
}

- (void)enqueueKeyEvent:(NSEvent*)event action:(uint32_t)action {
  __attribute__((objc_precise_lifetime)) DarwinArtMetalView* kept_view = self;
  if (_inputAdmissionClosed) return;
  const unsigned short scan_code = event.keyCode;
  const uint32_t key_code = darwin_art::input::AndroidKeyCode(scan_code);
  if (key_code == 0) return;
  const uint64_t event_time_nanos = AndroidEventTimeNanos();
  // AppKit raises an exception for key-only properties on FlagsChanged.
  // Modifier transitions still produce Android keys, without text or repeat.
  const bool is_key_event = event.type == NSEventTypeKeyDown ||
                            event.type == NSEventTypeKeyUp;
  const bool is_repeat = is_key_event && event.isARepeat;
  if (action == 0 && !is_repeat) {
    _keyDownTimes[scan_code] = event_time_nanos;
    _keyRepeatCounts[scan_code] = 0;
  }
  const auto down = _keyDownTimes.find(scan_code);
  const uint64_t down_time_nanos =
      down == _keyDownTimes.end() ? event_time_nanos : down->second;
  uint32_t repeat_count = 0;
  if (action == 0 && is_repeat) {
    repeat_count = ++_keyRepeatCounts[scan_code];
  }
  NSString* characters = is_key_event ? event.characters : nil;
  const uint32_t unicode_char =
      characters.length == 0 ? 0 : [characters characterAtIndex:0];
  const DarwinArtKeyEventV1 packet{
        .version = 1,
        .size = static_cast<uint32_t>(sizeof(DarwinArtKeyEventV1)),
        .action = action,
        // KeyEvent.FLAG_FROM_SYSTEM, as set by Android InputDispatcher for a
        // connected physical keyboard.
        .flags = 0x8,
        .sequence = _nextKeySequence++,
        .event_time_nanos = event_time_nanos,
        .down_time_nanos = down_time_nanos,
        .key_code = key_code,
        .scan_code = darwin_art::input::AndroidScanCode(key_code),
        .meta_state = darwin_art::input::AndroidMetaState(event.modifierFlags),
        .repeat_count = repeat_count,
        .device_id = 1,
        .source = 0x101,
        .unicode_char = unicode_char,
  };
  const auto sink = SnapshotInputSink(_ownerSurface);
  if (action == 1) {
    _keyDownTimes.erase(scan_code);
    _keyRepeatCounts.erase(scan_code);
  }
  const DarwinArtSurfaceInputResult input_result =
      sink == nullptr
          ? DARWIN_ART_SURFACE_INPUT_NO_SINK
          : sink->sink.key(sink->sink.context, &packet);
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::fprintf(stderr,
                 "ART AppKit key action=%u sequence=%llu key_code=%u enqueue=%u\n",
                 packet.action, static_cast<unsigned long long>(packet.sequence),
                 packet.key_code, static_cast<unsigned int>(input_result));
  }
  if (input_result != DARWIN_ART_SURFACE_INPUT_QUEUED &&
      std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::fprintf(stderr,
                 "ART Android input sink key status=%u action=%u "
                 "sequence=%llu\n",
                 static_cast<unsigned int>(input_result), packet.action,
                 static_cast<unsigned long long>(packet.sequence));
  }
  [kept_view signalOwnerWake];
}

- (void)keyDown:(NSEvent*)event {
  [self enqueueKeyEvent:event action:0];
}

- (void)keyUp:(NSEvent*)event {
  [self enqueueKeyEvent:event action:1];
}

- (void)flagsChanged:(NSEvent*)event {
  const NSEventModifierFlags flag =
      darwin_art::input::ModifierFlagForKey(event.keyCode);
  if (flag == 0) return;
  const uint32_t action = (event.modifierFlags & flag) != 0 ? 0 : 1;
  [self enqueueKeyEvent:event action:action];
}

- (void)cancelPointerStream {
  // Even an inactive cancellation revokes a pending outer DOWN. Its own
  // cancellation is the single expected revision advance in duplicate-DOWN;
  // any nested cancel/close invalidates that outer operation.
  if (_inputRevision == std::numeric_limits<uint64_t>::max()) {
    _inputAdmissionClosed = YES;
  } else {
    ++_inputRevision;
  }
  if (!_pointerActive) return;
  __attribute__((objc_precise_lifetime)) DarwinArtMetalView* kept_view = self;
  const uint64_t event_time_nanos = AndroidEventTimeNanos();
  const DarwinArtPointerEventV2 packet{
        .version = 2,
        .size = static_cast<uint32_t>(sizeof(DarwinArtPointerEventV2)),
        .action = DARWIN_ART_POINTER_CANCEL,
        .flags = 0,
        .sequence = _nextPointerSequence++,
        .event_time_nanos = event_time_nanos,
        .down_time_nanos = _downTimeNanos,
        .pointer_id = 0,
        .pointer_count = 1,
        .x = 0.0f,
        .y = 0.0f,
        .raw_x = 0.0f,
        .raw_y = 0.0f,
        .pressure = 0.0f,
        .size_value = 0.0f,
  };
  const auto sink = SnapshotInputSink(_ownerSurface);
  // Commit the old host stream before external code can start a replacement.
  _pointerActive = NO;
  _downTimeNanos = 0;
  const DarwinArtSurfaceInputResult input_result =
      sink == nullptr
          ? DARWIN_ART_SURFACE_INPUT_NO_SINK
          : sink->sink.pointer(sink->sink.context, &packet);
  if (input_result != DARWIN_ART_SURFACE_INPUT_QUEUED &&
      std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::fprintf(stderr,
                 "ART Android input sink cancel status=%u sequence=%llu\n",
                 static_cast<unsigned int>(input_result),
                 static_cast<unsigned long long>(packet.sequence));
  }
  [kept_view signalOwnerWake];
}

@end
