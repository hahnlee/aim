#pragma once

#include <jni.h>

#include <cstddef>

#include "aim/aim.h"

namespace aim_frame_probe {

struct Dimensions {
  std::size_t width = 0;
  std::size_t height = 0;
};

void configure(void* host_context, aim_frame_callback_t callback);
void reset();
jboolean present(JNIEnv* env, jint width, jint height, jintArray argb);
void record_dimensions(jint width, jint height);
Dimensions dimensions();

}  // namespace aim_frame_probe
