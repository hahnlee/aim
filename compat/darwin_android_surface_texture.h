#pragma once

#include <jni.h>

#include <cstdint>

struct ASurfaceTexture;

extern "C" ASurfaceTexture* aim_android_surface_texture_create(
    uint32_t width, uint32_t height, int32_t format, uint32_t texture);
extern "C" void* aim_android_surface_texture_producer(
    ASurfaceTexture* surface_texture);
extern "C" void aim_android_surface_texture_set_default_size(
    ASurfaceTexture* surface_texture, uint32_t width, uint32_t height);
extern "C" void aim_android_surface_texture_abandon(
    ASurfaceTexture* surface_texture);
extern "C" jlong aim_android_surface_texture_acquire_producer(
    JNIEnv* env, jobject surface_texture);

namespace aim {
bool RegisterDarwinSurfaceTextureNatives(JNIEnv* env);
}
