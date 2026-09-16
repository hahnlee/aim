#include "../../compat/darwin_android_media_ndk.h"
#include "../../compat/darwin_angle_egl.h"
#include <media/NdkImageReader.h>
#include <android/hardware_buffer.h>
#import <Foundation/Foundation.h>
#include <cassert>
#include <cstdio>

template<class T> T Api(const char* name) {
  auto* address = darwin_art_android_media_ndk_symbol(name);
  assert(address != nullptr);
  return reinterpret_cast<T>(address);
}

int main() {
  @autoreleasepool {
    auto create = Api<decltype(&AImageReader_newWithUsage)>("AImageReader_newWithUsage");
    auto window = Api<decltype(&AImageReader_getWindow)>("AImageReader_getWindow");
    auto acquire = Api<decltype(&AImageReader_acquireNextImageAsync)>("AImageReader_acquireNextImageAsync");
    auto hardware = Api<decltype(&AImage_getHardwareBuffer)>("AImage_getHardwareBuffer");
    auto width = Api<decltype(&AImage_getWidth)>("AImage_getWidth");
    auto release = Api<decltype(&AImage_delete)>("AImage_delete");
    auto close = Api<decltype(&AImageReader_delete)>("AImageReader_delete");
    AImageReader* reader = nullptr;
    assert(create(16, 16, AIMAGE_FORMAT_RGBA_8888,
                  AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE, 2, &reader) == AMEDIA_OK);
    ANativeWindow* producer = nullptr;
    assert(window(reader, &producer) == AMEDIA_OK);
    AImage* image = nullptr;
    int acquire_fence = -1;
    assert(acquire(reader, &image, &acquire_fence) == AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE);
    // Exercise real native-window slots and IOSurface allocation, without opening
    // an AppKit window. This verifies transport identity, not rendered pixels.
    // More iterations than the producer's three slots detect a slot that was
    // delivered once but never made available again after AImage_delete.
    for (int iteration = 0; iteration != 7; ++iteration) {
      AHardwareBuffer* submitted = nullptr;
      void* slot = nullptr;
      int dequeue_fence = -1;
      assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
                 producer, &submitted, &slot, &dequeue_fence) == 0);
      assert(submitted != nullptr && slot != nullptr && dequeue_fence == -1);
      assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(producer, slot, -1) == 0);
      assert(acquire(reader, &image, &acquire_fence) == AMEDIA_OK);
      assert(image != nullptr && acquire_fence == -1);
      AHardwareBuffer* received = nullptr;
      assert(hardware(image, &received) == AMEDIA_OK && received == submitted);
      int32_t actual_width = 0;
      assert(width(image, &actual_width) == AMEDIA_OK && actual_width == 16);
      if (iteration != 6) {
        release(image);
        image = nullptr;
        continue;
      }
      close(reader);
      assert(width(image, &actual_width) == AMEDIA_ERROR_INVALID_OBJECT);
      release(image);
    }
    std::puts("real runtime ImageReader queue identity and close invalidation PASS");
  }
}
