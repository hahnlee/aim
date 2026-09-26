// ImageReader over the real producer window and IOSurface-backed hardware
// buffers: the NDK entry points an app binds (AImageReader_*, AImage_*) with
// the native-window owner and hardware buffer owner linked directly, as the
// product links them. No product dylib or private export is used.
#include <android/hardware_buffer.h>
#include <media/NdkImageReader.h>

#include <cstdio>
#include <cstdlib>

extern "C" int darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
    ANativeWindow* window, AHardwareBuffer** buffer, void** slot, int* fence);
extern "C" int darwin_art_android_ANativeWindow_queue_hardware_buffer(
    ANativeWindow* window, void* slot, int fence);

#define CHECK(condition)                                              \
  do {                                                                \
    if (!(condition)) {                                               \
      std::fprintf(stderr, "%s:%d: CHECK failed: %s\n", __FILE__,      \
                   __LINE__, #condition);                             \
      std::abort();                                                   \
    }                                                                 \
  } while (false)

int main() {
  AImageReader* reader = nullptr;
  CHECK(AImageReader_newWithUsage(16, 16, AIMAGE_FORMAT_RGBA_8888,
                                  AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE, 2,
                                  &reader) == AMEDIA_OK);
  ANativeWindow* producer = nullptr;
  CHECK(AImageReader_getWindow(reader, &producer) == AMEDIA_OK);
  AImage* image = nullptr;
  int acquire_fence = -1;
  CHECK(AImageReader_acquireNextImageAsync(reader, &image, &acquire_fence) ==
        AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE);
  // Real native-window slots and IOSurface allocation, without an AppKit
  // window: transport identity, not rendered pixels. More iterations than the
  // producer's three slots detect a slot that was delivered once but never
  // made available again after AImage_delete.
  for (int iteration = 0; iteration != 7; ++iteration) {
    AHardwareBuffer* submitted = nullptr;
    void* slot = nullptr;
    int dequeue_fence = -1;
    CHECK(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
              producer, &submitted, &slot, &dequeue_fence) == 0);
    CHECK(submitted != nullptr && slot != nullptr && dequeue_fence == -1);
    AHardwareBuffer_Desc desc{};
    AHardwareBuffer_describe(submitted, &desc);
    CHECK(desc.width == 16 && desc.height == 16 &&
          desc.format == AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM);
    CHECK(darwin_art_android_ANativeWindow_queue_hardware_buffer(producer, slot, -1) == 0);
    CHECK(AImageReader_acquireNextImageAsync(reader, &image, &acquire_fence) == AMEDIA_OK);
    CHECK(image != nullptr && acquire_fence == -1);
    AHardwareBuffer* received = nullptr;
    CHECK(AImage_getHardwareBuffer(image, &received) == AMEDIA_OK && received == submitted);
    int32_t width = 0;
    CHECK(AImage_getWidth(image, &width) == AMEDIA_OK && width == 16);
    if (iteration != 6) {
      AImage_delete(image);
      image = nullptr;
      continue;
    }
    AImageReader_delete(reader);
    CHECK(AImage_getWidth(image, &width) == AMEDIA_ERROR_INVALID_OBJECT);
    AImage_delete(image);
  }
  std::puts("ImageReader producer queue identity and close invalidation PASS");
}
