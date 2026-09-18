#include <arpa/inet.h>
#include <fcntl.h>
#include <mach-o/dyld.h>
#include <netinet/in.h>
#include <poll.h>
#include <pthread.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

#include <array>
#include <atomic>
#include <cmath>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include "art_method-inl.h"
#include "base/locks.h"
#include "base/mem_map.h"
#include "base/logging.h"
#include "class_linker.h"
#include "cmdline_types.h"
#include "darwin_art/darwin_art.h"
#include "darwin_art_bionic_dns.h"
#include "darwin_art_bionic_fs.h"
#include "darwin_art_bionic_socket_broker.h"
#include "darwin_android_jni_trampoline.h"
#include "darwin_framework_natives.h"
#include "darwin_provider_owners.h"
#include "darwin_hwui_gpu_mode.h"
#include "darwin_surface_bridge.h"
#include "runtime_filesystem_probe.h"
#include "runtime_network_probe.h"
#include "runtime_elf_probe.h"
#include "runtime_abi_probe.h"
#include "../runtime/art/process_state.h"
#include "runtime_frame_probe.h"
#include "darwin_icu_natives.h"
#include "darwin_libcore_natives.h"
#include "darwin_openjdk_natives.h"
#include "dex/art_dex_file_loader.h"
#include "handle_scope-inl.h"
#include "interpreter/unstarted_runtime.h"
#include "jni/java_vm_ext.h"
#include "jvalue.h"
#include "mirror/class-inl.h"
#include "mirror/class_loader.h"
#include "mirror/throwable.h"
#include "runtime.h"
#include "runtime_options.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"
#include "well_known_classes.h"

#if defined(DARWIN_ART_DIRECT_APK_RUNTIME)
#include "runtime_apk_graph.h"
#endif

#include "runtime_graphics_probe.h"
#include "runtime_graphics_gpu.h"
#include "graphics_fixture_state.h"

namespace darwin_art_graphics {

jboolean present_content(GraphicsState* state, JNIEnv* env, jclass, jobject view,
                               jint width, jint height) {
  if (state == nullptr) return JNI_FALSE;
  if (view == nullptr || width <= 0 || height <= 0 || width > 4096 ||
      height > 4096) {
    return JNI_FALSE;
  }
#if defined(DARWIN_ART_REAL_GRAPHICS)
  constexpr bool kHeadlessFixture = false;
#else
  constexpr bool kHeadlessFixture = true;
#endif
#if defined(DARWIN_ART_REAL_GRAPHICS)
  if (darwin_art::hwui_gpu_enabled()) {
    return present_gpu_content(state, env, view, width, height);
  }
  // The production graphics flavor is GPU-only. Keep the software probe
  // implementation below available to the headless/raster flavor, but do not
  // compile an accidental Bitmap/IOSurface fallback into the Metal dylib.
  return JNI_FALSE;
#else
  jclass canvas_class = nullptr;
  jclass real_canvas_class = nullptr;
  jclass bitmap_class = nullptr;
  jclass bitmap_config_class = nullptr;
  jclass view_class = nullptr;
  jobject bitmap = nullptr;
  jobject canvas = nullptr;
  jintArray pixels = nullptr;
  jmethodID snapshot = nullptr;
  auto release_render_target = [&]() {
    env->DeleteLocalRef(pixels);
    env->DeleteLocalRef(view_class);
    env->DeleteLocalRef(canvas);
    env->DeleteLocalRef(bitmap);
    env->DeleteLocalRef(real_canvas_class);
    env->DeleteLocalRef(bitmap_config_class);
    env->DeleteLocalRef(bitmap_class);
  };
  if (kHeadlessFixture) {
    canvas_class =
        darwin_art_graphics_fixture::LookupProbeCanvasClass(state);
    if (canvas_class == nullptr) {
      std::cerr << "ART Android view: ProbeCanvas class is not rooted\n";
      return JNI_FALSE;
    }
    art::ScopedObjectAccess soa(env);
    art::ObjPtr<art::mirror::Class> canvas_mirror =
        soa.Decode<art::mirror::Class>(canvas_class);
    canvas = soa.AddLocalReference<jobject>(canvas_mirror->AllocObject(soa.Self()));
    jmethodID initialize =
        canvas == nullptr || env->ExceptionCheck()
            ? nullptr
            : env->GetMethodID(canvas_class, "initialize", "(II)V");
    if (!env->ExceptionCheck()) {
      snapshot = env->GetMethodID(canvas_class, "snapshot", "()[I");
    }
    if (canvas != nullptr && initialize != nullptr && snapshot != nullptr &&
        !env->ExceptionCheck()) {
      env->CallVoidMethod(canvas, initialize, width, height);
    }
  } else {
    bitmap_class = env->FindClass("android/graphics/Bitmap");
    if (!env->ExceptionCheck()) {
      bitmap_config_class = env->FindClass("android/graphics/Bitmap$Config");
    }
    if (!env->ExceptionCheck()) {
      real_canvas_class = env->FindClass("android/graphics/Canvas");
    }
    canvas_class = real_canvas_class;
    jfieldID argb_8888 = nullptr;
    jobject bitmap_config = nullptr;
    jmethodID create_bitmap = nullptr;
    jmethodID canvas_constructor = nullptr;
    if (bitmap_class != nullptr && bitmap_config_class != nullptr &&
        canvas_class != nullptr && !env->ExceptionCheck()) {
      argb_8888 = env->GetStaticFieldID(
          bitmap_config_class, "ARGB_8888",
          "Landroid/graphics/Bitmap$Config;");
    }
    if (argb_8888 != nullptr && !env->ExceptionCheck()) {
      bitmap_config =
          env->GetStaticObjectField(bitmap_config_class, argb_8888);
    }
    if (bitmap_config != nullptr && !env->ExceptionCheck()) {
      create_bitmap = env->GetStaticMethodID(
          bitmap_class, "createBitmap",
          "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;");
    }
    if (create_bitmap != nullptr && !env->ExceptionCheck()) {
      bitmap = env->CallStaticObjectMethod(bitmap_class, create_bitmap, width,
                                           height, bitmap_config);
    }
    if (bitmap != nullptr && !env->ExceptionCheck()) {
      canvas_constructor = env->GetMethodID(
          canvas_class, "<init>", "(Landroid/graphics/Bitmap;)V");
    }
    if (canvas_constructor != nullptr && !env->ExceptionCheck()) {
      canvas = env->NewObject(canvas_class, canvas_constructor, bitmap);
    }
    env->DeleteLocalRef(bitmap_config);
  }
  const bool real_target_missing = !kHeadlessFixture && bitmap == nullptr;
  if (canvas == nullptr || real_target_missing || env->ExceptionCheck()) {
    std::cerr << "ART Android view: "
              << (!kHeadlessFixture
                      ? "Bitmap/Canvas(Bitmap) setup failed\n"
                      : "ProbeCanvas setup failed\n");
    art::Thread* self = art::Thread::Current();
    if (self != nullptr && self->IsExceptionPending()) {
      std::cerr << self->GetException()->Dump() << "\n";
    }
    release_render_target();
    return JNI_FALSE;
  }
  view_class = env->FindClass("android/view/View");
  auto get_view_method = [&](const char* name,
                             const char* signature) -> jmethodID {
    return view_class == nullptr || env->ExceptionCheck()
               ? nullptr
               : env->GetMethodID(view_class, name, signature);
  };
  auto get_view_field = [&](const char* name) -> jfieldID {
    return view_class == nullptr || env->ExceptionCheck()
               ? nullptr
               : env->GetFieldID(view_class, name, "I");
  };
  jmethodID layout =
      get_view_method("layout", "(IIII)V");
  jmethodID measure =
      get_view_method("measure", "(II)V");
  jmethodID draw =
      get_view_method("draw", "(Landroid/graphics/Canvas;)V");
  jfieldID view_left = get_view_field("mLeft");
  jfieldID view_top = get_view_field("mTop");
  jfieldID view_right = get_view_field("mRight");
  jfieldID view_bottom = get_view_field("mBottom");
  if (canvas == nullptr || measure == nullptr || layout == nullptr ||
      draw == nullptr || view_left == nullptr || view_top == nullptr ||
      view_right == nullptr || view_bottom == nullptr ||
      env->ExceptionCheck()) {
    release_render_target();
    return JNI_FALSE;
  }

  constexpr jint kExactly = 0x40000000;
  env->CallVoidMethod(view, measure, kExactly | width, kExactly | height);
  if (env->ExceptionCheck()) {
    release_render_target();
    return JNI_FALSE;
  }
  // ViewRoot normally installs the surface bounds before the first traversal.
  // The Darwin window policy owns that root, so seed the same bounds before
  // layout. This prevents a detached View from trying to notify Android's
  // accessibility/window services merely because its initial frame changed.
  env->SetIntField(view, view_left, 0);
  env->SetIntField(view, view_top, 0);
  env->SetIntField(view, view_right, width);
  env->SetIntField(view, view_bottom, height);
  if (env->ExceptionCheck()) {
    release_render_target();
    return JNI_FALSE;
  }
  env->CallVoidMethod(view, layout, 0, 0, width, height);
  if (env->ExceptionCheck()) {
    release_render_target();
    return JNI_FALSE;
  }
  env->CallVoidMethod(view, draw, canvas);
  if (env->ExceptionCheck()) {
    release_render_target();
    return JNI_FALSE;
  }
  if (kHeadlessFixture) {
    pixels = static_cast<jintArray>(env->CallObjectMethod(canvas, snapshot));
  } else {
    const jsize pixel_count = static_cast<jsize>(width * height);
    pixels = env->NewIntArray(pixel_count);
    jmethodID get_pixels =
        bitmap_class == nullptr || env->ExceptionCheck()
            ? nullptr
            : env->GetMethodID(bitmap_class, "getPixels", "([IIIIIII)V");
    if (pixels != nullptr && get_pixels != nullptr && !env->ExceptionCheck()) {
      env->CallVoidMethod(bitmap, get_pixels, pixels, 0, width, 0, 0, width,
                          height);
    }
  }
  if (env->ExceptionCheck() || pixels == nullptr) {
    release_render_target();
    return JNI_FALSE;
  }
  const jboolean presented =
      darwin_art_frame_probe::present(env, width, height, pixels);
  release_render_target();
  return presented;
#endif
}

}  // namespace darwin_art_graphics
