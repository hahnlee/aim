#include "media_codec_surface_fixture.h"

#include "../compat/darwin_angle_egl.h"

#include <cstdlib>
#include <iostream>

namespace darwin_art::media_fixture {
namespace {

bool DebugFixture() {
  return std::getenv("DARWIN_ART_DEBUG_MEDIA_CODEC") != nullptr;
}

void Trace(const char* stage) {
  if (DebugFixture())
    std::cerr << "ART Android MediaCodec fixture: " << stage << "\n";
}

}  // namespace

bool VerifyMediaCodecSurfaceLifecycle(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  Trace("begin");
  jclass surface_class = env->FindClass("android/view/Surface");
  jmethodID surface_constructor =
      surface_class == nullptr
          ? nullptr
          : env->GetMethodID(surface_class, "<init>", "()V");
  jfieldID native_object =
      surface_class == nullptr
          ? nullptr
          : env->GetFieldID(surface_class, "mNativeObject", "J");
  jmethodID surface_release =
      surface_class == nullptr
          ? nullptr
          : env->GetMethodID(surface_class, "release", "()V");
  jobject first_surface = surface_constructor == nullptr
                              ? nullptr
                              : env->NewObject(surface_class,
                                               surface_constructor);
  jobject second_surface = surface_constructor == nullptr
                               ? nullptr
                               : env->NewObject(surface_class,
                                                surface_constructor);
  void* first_window = darwin_art_android_ANativeWindow_create(32, 24, 1);
  void* second_window = darwin_art_android_ANativeWindow_create(32, 24, 1);
  if (first_surface == nullptr || second_surface == nullptr ||
      native_object == nullptr || surface_release == nullptr ||
      first_window == nullptr || second_window == nullptr ||
      env->ExceptionCheck()) {
    if (first_window != nullptr)
      darwin_art_android_ANativeWindow_release(first_window);
    if (second_window != nullptr)
      darwin_art_android_ANativeWindow_release(second_window);
    return false;
  }
  Trace("surfaces-created");
  env->SetLongField(first_surface, native_object,
                    reinterpret_cast<jlong>(first_window));
  env->SetLongField(second_surface, native_object,
                    reinterpret_cast<jlong>(second_window));

  jclass codec_class = env->FindClass("android/media/MediaCodec");
  jmethodID create_decoder =
      codec_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(
                codec_class, "createDecoderByType",
                "(Ljava/lang/String;)Landroid/media/MediaCodec;");
  jmethodID configure =
      codec_class == nullptr
          ? nullptr
          : env->GetMethodID(
                codec_class, "configure",
                "(Landroid/media/MediaFormat;Landroid/view/Surface;"
                "Landroid/media/MediaCrypto;I)V");
  jmethodID set_output_surface =
      codec_class == nullptr
          ? nullptr
          : env->GetMethodID(codec_class, "setOutputSurface",
                             "(Landroid/view/Surface;)V");
  jmethodID codec_release =
      codec_class == nullptr
          ? nullptr
          : env->GetMethodID(codec_class, "release", "()V");
  jclass format_class = env->FindClass("android/media/MediaFormat");
  jmethodID create_video_format =
      format_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(
                format_class, "createVideoFormat",
                "(Ljava/lang/String;II)Landroid/media/MediaFormat;");
  jstring mime = env->NewStringUTF("video/x-vnd.on2.vp9");
  jobject codec = create_decoder == nullptr || mime == nullptr
                      ? nullptr
                      : env->CallStaticObjectMethod(codec_class,
                                                    create_decoder, mime);
  jobject format = create_video_format == nullptr || mime == nullptr
                       ? nullptr
                       : env->CallStaticObjectMethod(format_class,
                                                     create_video_format, mime,
                                                     32, 24);
  bool passed = codec != nullptr && format != nullptr && configure != nullptr &&
                set_output_surface != nullptr && codec_release != nullptr &&
                !env->ExceptionCheck();
  Trace("codec-and-format-created");
  if (passed) {
    Trace("configure-enter");
    env->CallVoidMethod(codec, configure, format, first_surface, nullptr, 0);
    Trace("configure-returned");
    passed = !env->ExceptionCheck() &&
             darwin_art_android_ANativeWindow_is_managed(first_window);
  }
  if (passed) {
    Trace("first-surface-release-enter");
    env->CallVoidMethod(first_surface, surface_release);
    Trace("first-surface-release-returned");
    passed = !env->ExceptionCheck() &&
             darwin_art_android_ANativeWindow_is_managed(first_window);
  }
  if (passed) {
    Trace("set-output-surface-enter");
    env->CallVoidMethod(codec, set_output_surface, second_surface);
    Trace("set-output-surface-returned");
    passed = !env->ExceptionCheck() &&
             darwin_art_android_ANativeWindow_is_managed(second_window) &&
             !darwin_art_android_ANativeWindow_is_managed(first_window);
  }
  if (passed) {
    Trace("second-surface-release-enter");
    env->CallVoidMethod(second_surface, surface_release);
    Trace("second-surface-release-returned");
    passed = !env->ExceptionCheck() &&
             darwin_art_android_ANativeWindow_is_managed(second_window);
  }
  if (codec != nullptr && codec_release != nullptr && !env->ExceptionCheck()) {
    Trace("codec-release-enter");
    env->CallVoidMethod(codec, codec_release);
    Trace("codec-release-returned");
    passed = passed && !env->ExceptionCheck() &&
             !darwin_art_android_ANativeWindow_is_managed(second_window);
  }
  if (env->ExceptionCheck()) {
    env->ExceptionDescribe();
    env->ExceptionClear();
    passed = false;
  }
  if (format != nullptr) env->DeleteLocalRef(format);
  if (codec != nullptr) env->DeleteLocalRef(codec);
  if (mime != nullptr) env->DeleteLocalRef(mime);
  if (format_class != nullptr) env->DeleteLocalRef(format_class);
  if (codec_class != nullptr) env->DeleteLocalRef(codec_class);
  if (second_surface != nullptr) env->DeleteLocalRef(second_surface);
  if (first_surface != nullptr) env->DeleteLocalRef(first_surface);
  if (surface_class != nullptr) env->DeleteLocalRef(surface_class);
  if (passed)
    std::cerr << "ART Android MediaCodec: setOutputSurface producer lifetime PASS\n";
  return passed;
}

}  // namespace darwin_art::media_fixture
