#include "runtime_graphics_phase.h"

#include "graphics_fixture_state.h"

#include <cstddef>
#include <iostream>
#include <string>

#include "runtime_frame_probe.h"
#include "runtime_graphics_probe.h"

namespace darwin_art_graphics_phase {

int present_and_retain(darwin_art_graphics::GraphicsState* state,
                       JNIEnv* env, jobject decor_view,
                       jclass content_root_class, jobject content_root,
                       jclass probe_view_class, jobject probe_view,
                       bool run_apk_app, bool expect_apk_widgets,
                       bool retain_interactive, jint width, jint height) {
  if (env == nullptr || decor_view == nullptr || content_root == nullptr) {
    std::cerr << "ART Android graphics: presentation inputs are missing\n";
    return 33;
  }

  if (expect_apk_widgets) {
    static constexpr const char* kWidgetTags[] = {
        "title", "checkbox", "radio", "toggle", "seek", "progress",
        "button"};
    static constexpr const char* kWidgetClasses[] = {
        "android/widget/TextView",     "android/widget/CheckBox",
        "android/widget/RadioButton", "android/widget/ToggleButton",
        "android/widget/SeekBar",     "android/widget/ProgressBar",
        "android/widget/Button"};
    jmethodID find_view_with_tag =
        content_root_class == nullptr
            ? nullptr
            : env->GetMethodID(content_root_class, "findViewWithTag",
                               "(Ljava/lang/Object;)Landroid/view/View;");
    bool widgets_valid = find_view_with_tag != nullptr;
    for (std::size_t index = 0;
         widgets_valid && index < sizeof(kWidgetTags) / sizeof(kWidgetTags[0]);
         ++index) {
      jstring tag = env->NewStringUTF(kWidgetTags[index]);
      jobject widget =
          tag == nullptr
              ? nullptr
              : env->CallObjectMethod(content_root, find_view_with_tag, tag);
      jclass widget_class = env->FindClass(kWidgetClasses[index]);
      widgets_valid = tag != nullptr && widget != nullptr &&
                      widget_class != nullptr &&
                      env->IsInstanceOf(widget, widget_class) &&
                      !env->ExceptionCheck();
      if (widget_class != nullptr) env->DeleteLocalRef(widget_class);
      if (widget != nullptr) env->DeleteLocalRef(widget);
      if (tag != nullptr) env->DeleteLocalRef(tag);
    }
    if (widgets_valid) {
      jstring title_tag = env->NewStringUTF("title");
      jobject title_view = env->CallObjectMethod(content_root, find_view_with_tag,
                                                 title_tag);
      jclass text_view_class = env->FindClass("android/widget/TextView");
      jmethodID get_text = text_view_class == nullptr
                               ? nullptr
                               : env->GetMethodID(text_view_class, "getText",
                                                  "()Ljava/lang/CharSequence;");
      jobject title_text =
          title_view == nullptr || get_text == nullptr
              ? nullptr
              : env->CallObjectMethod(title_view, get_text);
      jclass object_class = env->FindClass("java/lang/Object");
      jmethodID to_string = object_class == nullptr
                                ? nullptr
                                : env->GetMethodID(object_class, "toString",
                                                   "()Ljava/lang/String;");
      jstring title_string = title_text == nullptr || to_string == nullptr
                                 ? nullptr
                                 : static_cast<jstring>(env->CallObjectMethod(
                                       title_text, to_string));
      const char* title_chars = title_string == nullptr
                                    ? nullptr
                                    : env->GetStringUTFChars(title_string, nullptr);
      widgets_valid = title_chars != nullptr &&
                      std::string(title_chars) == "AOSP resource-backed widgets" &&
                      !env->ExceptionCheck();
      if (title_chars != nullptr) {
        env->ReleaseStringUTFChars(title_string, title_chars);
      }
      env->DeleteLocalRef(title_string);
      env->DeleteLocalRef(object_class);
      env->DeleteLocalRef(title_text);
      env->DeleteLocalRef(text_view_class);
      env->DeleteLocalRef(title_view);
      env->DeleteLocalRef(title_tag);
    }
    if (!widgets_valid) {
      std::cerr << "ART Android APK: framework widget set is incomplete\n";
      return 33;
    }
  }

  // A real APK's application content lives under android.R.id.content. The
  // window background remains owned by the CAMetal clear, while this root
  // records every Calculator sibling (display + keypad) in one Android view
  // traversal. The isolated button probe still uses its bounded child.
  jobject render_root = run_apk_app
                            ? content_root
                            : (probe_view != nullptr ? probe_view : content_root);
  const jboolean decor_presented =
      env->ExceptionCheck()
          ? JNI_FALSE
          : darwin_art_graphics::present_content(
                state, env, nullptr, render_root, width, height);
  std::cerr << "ART Android graphics: present root=" << render_root
            << " apk=" << (run_apk_app ? 1 : 0)
            << " result=" << (decor_presented == JNI_TRUE ? 1 : 0)
            << " exception=" << (env->ExceptionCheck() ? 1 : 0)
            << " size=" << width << "x" << height << "\n";
  jmethodID was_presented =
      run_apk_app || probe_view_class == nullptr
          ? nullptr
          : env->GetMethodID(probe_view_class, "wasPresented", "()Z");
  const jboolean view_presented =
      run_apk_app
          ? decor_presented
          : (decor_presented != JNI_TRUE || probe_view == nullptr ||
                     !env->IsInstanceOf(probe_view, probe_view_class) ||
                     was_presented == nullptr || env->ExceptionCheck()
                 ? JNI_FALSE
                 : env->CallBooleanMethod(probe_view, was_presented));
  const auto dimensions = darwin_art_frame_probe::dimensions();
  if (view_presented != JNI_TRUE || dimensions.width !=
                                         static_cast<std::size_t>(width) ||
      dimensions.height != static_cast<std::size_t>(height) ||
      env->ExceptionCheck()) {
    std::cerr << "ART Android view: Activity content presentation failed\n";
    return 33;
  }

  if (retain_interactive &&
      !darwin_art_graphics_fixture::retain_interactive_root(state, env,
                                                            decor_view, width,
                                                            height)) {
    std::cerr << "ART Android input: retaining DecorView failed\n";
    return 33;
  }
  return 0;
}

}  // namespace darwin_art_graphics_phase
