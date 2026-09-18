#pragma once

#include <jni.h>

#include "../runtime/embedding/graphics_state.h"

namespace art {
class Thread;
}

namespace darwin_art_presentation {

// Exercises Android's public Surface software-Canvas contract against a real
// managed producer, including native lock, Canvas binding, post, and release.
bool verify_software_surface_canvas(JNIEnv* env);

// Builds the detached Activity/PhoneWindow/DecorView hierarchy and presents its
// Android-owned frame. All references created by this operation are released
// before returning; the caller retains only the app bootstrap classes it owns.
int run(JNIEnv* env, art::Thread* self, jobject activity_instance,
         jclass probe_activity_class, jclass probe_context_class,
         jclass probe_resources_class, jclass probe_view_class,
         jclass probe_canvas_class, jclass content_root_class,
         jobject package_manager, bool run_apk_app,
         bool use_framework_resources, bool expect_apk_widgets,
         bool apk_native_loaded, bool run_framework_button, jint window_scale,
         const char* framework_res_apk, const char* apk_app_package,
         const char* apk_app_activity, const char* app_apk_path,
         darwin_art_graphics::GraphicsState* graphics_state);

// Boots an APK Application and one manifest Service without creating a
// Window, then serves its Binder on the inherited isolated-process channel.
int run_service(JNIEnv* env, art::Thread* self, jclass service_class,
                jclass probe_context_class, jclass probe_resources_class,
                jobject package_manager, bool use_framework_resources,
                jint window_scale, const char* framework_res_apk,
                const char* apk_app_package, const char* service_class_name,
                const char* app_apk_path, jint control_fd);

}  // namespace darwin_art_presentation
