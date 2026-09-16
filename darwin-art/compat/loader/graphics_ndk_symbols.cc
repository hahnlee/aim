#include "graphics_ndk_image.h"
#include <android/bitmap.h>
#include <android/imagedecoder.h>
#include <cstring>

namespace darwin_art::loader {
uintptr_t GraphicsNdkSymbol(const char* symbol) {
  if (!symbol) return 0;
  // Exact original libjnigraphics.map.txt surface. Missing code fails linkage;
  // no host dlsym fallback or fabricated success entry is permitted.
#define PUBLIC_NDK(name) if (std::strcmp(symbol, #name) == 0) return reinterpret_cast<uintptr_t>(&name)
  PUBLIC_NDK(AImageDecoder_resultToString);
  PUBLIC_NDK(AImageDecoder_createFromAAsset);
  PUBLIC_NDK(AImageDecoder_createFromFd);
  PUBLIC_NDK(AImageDecoder_createFromBuffer);
  PUBLIC_NDK(AImageDecoder_delete);
  PUBLIC_NDK(AImageDecoder_setAndroidBitmapFormat);
  PUBLIC_NDK(AImageDecoder_setUnpremultipliedRequired);
  PUBLIC_NDK(AImageDecoder_setDataSpace);
  PUBLIC_NDK(AImageDecoder_getHeaderInfo);
  PUBLIC_NDK(AImageDecoder_getMinimumStride);
  PUBLIC_NDK(AImageDecoder_decodeImage);
  PUBLIC_NDK(AImageDecoder_setTargetSize);
  PUBLIC_NDK(AImageDecoder_computeSampledSize);
  PUBLIC_NDK(AImageDecoder_setCrop);
  PUBLIC_NDK(AImageDecoder_isAnimated);
  PUBLIC_NDK(AImageDecoder_getRepeatCount);
  PUBLIC_NDK(AImageDecoder_advanceFrame);
  PUBLIC_NDK(AImageDecoder_rewind);
  PUBLIC_NDK(AImageDecoder_getFrameInfo);
  PUBLIC_NDK(AImageDecoder_setInternallyHandleDisposePrevious);
  PUBLIC_NDK(AImageDecoderHeaderInfo_getWidth);
  PUBLIC_NDK(AImageDecoderHeaderInfo_getHeight);
  PUBLIC_NDK(AImageDecoderHeaderInfo_getMimeType);
  PUBLIC_NDK(AImageDecoderHeaderInfo_getAlphaFlags);
  PUBLIC_NDK(AImageDecoderHeaderInfo_getAndroidBitmapFormat);
  PUBLIC_NDK(AImageDecoderHeaderInfo_getDataSpace);
  PUBLIC_NDK(AImageDecoderFrameInfo_create);
  PUBLIC_NDK(AImageDecoderFrameInfo_delete);
  PUBLIC_NDK(AImageDecoderFrameInfo_getDuration);
  PUBLIC_NDK(AImageDecoderFrameInfo_getFrameRect);
  PUBLIC_NDK(AImageDecoderFrameInfo_hasAlphaWithinBounds);
  PUBLIC_NDK(AImageDecoderFrameInfo_getDisposeOp);
  PUBLIC_NDK(AImageDecoderFrameInfo_getBlendOp);
  PUBLIC_NDK(AndroidBitmap_getInfo);
  PUBLIC_NDK(AndroidBitmap_getDataSpace);
  PUBLIC_NDK(AndroidBitmap_lockPixels);
  PUBLIC_NDK(AndroidBitmap_unlockPixels);
  PUBLIC_NDK(AndroidBitmap_compress);
  PUBLIC_NDK(AndroidBitmap_getHardwareBuffer);
#undef PUBLIC_NDK
  return 0;
}
}
