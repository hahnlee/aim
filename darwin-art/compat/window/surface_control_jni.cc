#include "surface_control_jni.h"
#include "surface_transaction_submission.h"
#include "../../runtime/framework/wm/desktop_window_metadata.h"
#include "../darwin_android_platform.h"
#include <android/surface_control.h>
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <iterator>
#include <limits>

struct ASurfaceControl;
extern "C" void ASurfaceControl_acquire(ASurfaceControl*);

namespace {
[[noreturn]] void InvalidSurfaceControl() {
  std::fputs("ASurfaceControl_fromJava requires a live android.view.SurfaceControl\n", stderr);
  std::abort();
}
}

// Android native/android/surface_control.cpp retains the control owned by the
// supplied Java object. Never manufacture a second layer for this conversion.
// Native storage still belongs to the existing SurfaceControl owner; replacing
// that owner with libgui must update the retain implementation as one contract.
extern "C" ASurfaceControl* ASurfaceControl_fromJava(JNIEnv* env, jobject object) {
  if (!env || !object) InvalidSurfaceControl();
  if (env->ExceptionCheck()) return nullptr;
  jclass type = env->FindClass("android/view/SurfaceControl");
  if (!type) return nullptr;  // Preserve the VM's class-resolution exception.
  if (!env->IsInstanceOf(object, type)) {
    env->DeleteLocalRef(type);
    InvalidSurfaceControl();
  }
  jfieldID field = env->GetFieldID(type, "mNativeObject", "J");
  env->DeleteLocalRef(type);
  if (!field || env->ExceptionCheck()) return nullptr;
  jlong native = env->GetLongField(object, field);
  if (env->ExceptionCheck()) return nullptr;
  if (!native) InvalidSurfaceControl();
  auto* control = reinterpret_cast<ASurfaceControl*>(static_cast<uintptr_t>(native));
  ASurfaceControl_acquire(control);
  return control;
}

namespace {

void SurfaceControlFinalizer(void* control) {
  ASurfaceControl_release(reinterpret_cast<ASurfaceControl*>(control));
}

void SurfaceTransactionFinalizer(void* transaction) {
  ASurfaceTransaction_delete(
      reinterpret_cast<ASurfaceTransaction*>(transaction));
}

jlong SurfaceControlNativeGetFinalizer(JNIEnv*, jclass) {
  return reinterpret_cast<jlong>(&SurfaceControlFinalizer);
}

jlong SurfaceTransactionNativeGetFinalizer(JNIEnv*, jclass) {
  return reinterpret_cast<jlong>(&SurfaceTransactionFinalizer);
}

jlong SurfaceControlNativeCreate(JNIEnv* env, jclass, jobject, jstring name,
                                 jint, jint, jint, jint, jlong parent,
                                 jobject) {
  if (!darwin_art::framework::wm::EnsureDesktopWindowMetadataReceiver(env)) {
    return 0;
  }
  const char* utf =
      name == nullptr ? nullptr : env->GetStringUTFChars(name, nullptr);
  ASurfaceControl* control =
      parent == 0
          ? reinterpret_cast<ASurfaceControl*>(
                darwin_art_android_surface_control_create_root(
                    utf == nullptr ? "SurfaceControl" : utf))
          : ASurfaceControl_create(reinterpret_cast<ASurfaceControl*>(parent),
                                   utf == nullptr ? "SurfaceControl" : utf);
  if (utf != nullptr) env->ReleaseStringUTFChars(name, utf);
  return reinterpret_cast<jlong>(control);
}

jlong SurfaceControlNativeGetHandle(JNIEnv*, jclass, jlong native_object) {
  // SurfaceControl's public handle is a stable reference to the same native
  // layer identity; Binder serialization is supplied by the compositor bridge.
  return native_object;
}

jlong SurfaceControlNativeCopy(JNIEnv*, jclass, jlong native_object) {
  auto* control = reinterpret_cast<ASurfaceControl*>(native_object);
  if (control != nullptr) ASurfaceControl_acquire(control);
  return native_object;
}

constexpr jlong kDarwinSurfaceControlParcelMagic =
    static_cast<jlong>(0x4441534300000001ull);

void SurfaceControlNativeWriteToParcel(JNIEnv* env, jclass, jlong native_object,
                                       jobject parcel) {
  if (env == nullptr || parcel == nullptr || native_object == 0) return;
  uint32_t owner_process_id = 0;
  uint32_t layer_id = 0;
  if (!darwin_art_android_surface_control_get_identity(
          reinterpret_cast<void*>(static_cast<uintptr_t>(native_object)),
          &owner_process_id, &layer_id)) {
    return;
  }
  jclass parcel_class = env->GetObjectClass(parcel);
  jmethodID write_long =
      parcel_class == nullptr
          ? nullptr
          : env->GetMethodID(parcel_class, "writeLong", "(J)V");
  jmethodID write_int =
      parcel_class == nullptr
          ? nullptr
          : env->GetMethodID(parcel_class, "writeInt", "(I)V");
  if (write_long != nullptr && write_int != nullptr &&
      !env->ExceptionCheck()) {
    env->CallVoidMethod(parcel, write_long, kDarwinSurfaceControlParcelMagic);
    env->CallVoidMethod(parcel, write_int,
                        static_cast<jint>(owner_process_id));
    env->CallVoidMethod(parcel, write_int, static_cast<jint>(layer_id));
  }
  if (parcel_class != nullptr) env->DeleteLocalRef(parcel_class);
}

jlong SurfaceControlNativeReadFromParcel(JNIEnv* env, jclass, jobject parcel) {
  if (env == nullptr || parcel == nullptr) return 0;
  jclass parcel_class = env->GetObjectClass(parcel);
  jmethodID read_long =
      parcel_class == nullptr
          ? nullptr
          : env->GetMethodID(parcel_class, "readLong", "()J");
  jmethodID read_int =
      parcel_class == nullptr
          ? nullptr
          : env->GetMethodID(parcel_class, "readInt", "()I");
  if (read_long == nullptr || read_int == nullptr || env->ExceptionCheck()) {
    if (parcel_class != nullptr) env->DeleteLocalRef(parcel_class);
    return 0;
  }
  const jlong magic = env->CallLongMethod(parcel, read_long);
  const jint owner_process_id = env->CallIntMethod(parcel, read_int);
  const jint layer_id = env->CallIntMethod(parcel, read_int);
  if (parcel_class != nullptr) env->DeleteLocalRef(parcel_class);
  if (env->ExceptionCheck() || magic != kDarwinSurfaceControlParcelMagic ||
      owner_process_id <= 0 || layer_id <= 0) {
    return 0;
  }
  return reinterpret_cast<jlong>(
      darwin_art_android_surface_control_create_imported(
          static_cast<uint32_t>(owner_process_id),
          static_cast<uint32_t>(layer_id), "Imported SurfaceControl"));
}

void SurfaceControlNativeDisconnect(JNIEnv*, jclass, jlong) {}

jlong SurfaceControlNativeCreateTransaction(JNIEnv*, jclass) {
  return reinterpret_cast<jlong>(ASurfaceTransaction_create());
}

void SurfaceControlNativeSetTransformHint(JNIEnv*, jclass, jlong, jint) {}
void SurfaceControlNativeSetFrameRateCategory(JNIEnv*, jclass, jlong, jlong,
                                              jint, jboolean) {}

void SurfaceControlNativeClearTransaction(JNIEnv*, jclass, jlong transaction) {
  darwin_art_android_surface_transaction_clear(
      reinterpret_cast<void*>(transaction));
}

void SurfaceControlNativeMergeTransaction(JNIEnv*, jclass, jlong destination,
                                          jlong source) {
  darwin_art_android_surface_transaction_merge(
      reinterpret_cast<void*>(destination), reinterpret_cast<void*>(source));
}

void SurfaceControlNativeTransactionNoop1(JNIEnv*, jclass, jlong) {}
void SurfaceControlNativeTransactionNoop2(JNIEnv*, jclass, jlong, jlong) {}
void SurfaceControlNativeTransactionNoop3(JNIEnv*, jclass, jlong, jlong,
                                          jlong) {}

void SurfaceControlNativeSetTransparentRegionHint(JNIEnv* env, jclass,
                                                  jlong transaction,
                                                  jlong control,
                                                  jobject region) {
  if (env == nullptr) return;
  // RegionIterator exposes the exact SkRegion rectangles used by ViewRoot's
  // transparent-region hint. Keep the producer ABI bounded to eight rects;
  // an empty/null Region explicitly clears the prior hint.
  std::array<int32_t, 32> flattened{};
  size_t count = 0;
  if (region != nullptr) {
    jclass iterator_class = env->FindClass("android/graphics/RegionIterator");
    jclass rect_class = env->FindClass("android/graphics/Rect");
    jmethodID iterator_constructor =
        iterator_class == nullptr
            ? nullptr
            : env->GetMethodID(iterator_class, "<init>",
                               "(Landroid/graphics/Region;)V");
    jmethodID next =
        iterator_class == nullptr
            ? nullptr
            : env->GetMethodID(iterator_class, "next",
                               "(Landroid/graphics/Rect;)Z");
    jmethodID rect_constructor =
        rect_class == nullptr
            ? nullptr
            : env->GetMethodID(rect_class, "<init>", "()V");
    jfieldID left = rect_class == nullptr
                        ? nullptr
                        : env->GetFieldID(rect_class, "left", "I");
    jfieldID top = rect_class == nullptr
                       ? nullptr
                       : env->GetFieldID(rect_class, "top", "I");
    jfieldID right = rect_class == nullptr
                         ? nullptr
                         : env->GetFieldID(rect_class, "right", "I");
    jfieldID bottom = rect_class == nullptr
                          ? nullptr
                          : env->GetFieldID(rect_class, "bottom", "I");
    jobject iterator =
        iterator_constructor == nullptr
            ? nullptr
            : env->NewObject(iterator_class, iterator_constructor, region);
    jobject rect = rect_constructor == nullptr
                       ? nullptr
                       : env->NewObject(rect_class, rect_constructor);
    const bool valid = iterator != nullptr && rect != nullptr && next != nullptr &&
                       left != nullptr && top != nullptr && right != nullptr &&
                       bottom != nullptr && !env->ExceptionCheck();
    if (valid) {
      while (count < 8 &&
             env->CallBooleanMethod(iterator, next, rect) == JNI_TRUE) {
        const int32_t rect_left = env->GetIntField(rect, left);
        const int32_t rect_top = env->GetIntField(rect, top);
        const int32_t rect_right = env->GetIntField(rect, right);
        const int32_t rect_bottom = env->GetIntField(rect, bottom);
        if (rect_right > rect_left && rect_bottom > rect_top) {
          flattened[count * 4 + 0] = rect_left;
          flattened[count * 4 + 1] = rect_top;
          flattened[count * 4 + 2] = rect_right;
          flattened[count * 4 + 3] = rect_bottom;
          ++count;
        }
      }
    }
    const bool failed = env->ExceptionCheck();
    if (iterator != nullptr) env->DeleteLocalRef(iterator);
    if (rect != nullptr) env->DeleteLocalRef(rect);
    if (iterator_class != nullptr) env->DeleteLocalRef(iterator_class);
    if (rect_class != nullptr) env->DeleteLocalRef(rect_class);
    if (failed) {
      env->ExceptionClear();
      return;
    }
  }
  darwin_art_android_surface_transaction_set_transparent_region_hint(
      reinterpret_cast<void*>(transaction), reinterpret_cast<void*>(control),
      flattened.data(), count);
}

ASurfaceTransaction* SurfaceTransaction(jlong handle) {
  return reinterpret_cast<ASurfaceTransaction*>(handle);
}

ASurfaceControl* SurfaceControl(jlong handle) {
  return reinterpret_cast<ASurfaceControl*>(handle);
}

constexpr jint kSurfaceControlLayerHidden = 0x01;
constexpr bool SurfaceControlFlagsChangeVisibility(jint mask) {
  return (mask & kSurfaceControlLayerHidden) != 0;
}
static_assert(SurfaceControlFlagsChangeVisibility(0x01));
static_assert(!SurfaceControlFlagsChangeVisibility(0x02));  // OPAQUE
static_assert(!SurfaceControlFlagsChangeVisibility(0x40));  // SKIP_SCREENSHOT
static_assert(!SurfaceControlFlagsChangeVisibility(0x80));  // SECURE

void SurfaceControlNativeSetFlags(JNIEnv*, jclass, jlong transaction,
                                  jlong control, jint flags, jint mask) {
  // layer_state_t::eLayerHidden is bit 0. nativeSetFlags also carries
  // independent state such as OPAQUE, SECURE, SKIP_SCREENSHOT, and
  // backpressure; those bits must not change layer visibility.
  if (!SurfaceControlFlagsChangeVisibility(mask)) return;
  ASurfaceTransaction_setVisibility(
      SurfaceTransaction(transaction), SurfaceControl(control),
      (flags & kSurfaceControlLayerHidden) == 0
          ? ASURFACE_TRANSACTION_VISIBILITY_SHOW
          : ASURFACE_TRANSACTION_VISIBILITY_HIDE);
}

void SurfaceControlNativeSetPosition(JNIEnv*, jclass, jlong transaction,
                                     jlong control, jfloat x, jfloat y) {
  ASurfaceTransaction_setPosition(SurfaceTransaction(transaction),
                                  SurfaceControl(control),
                                  static_cast<int32_t>(std::lround(x)),
                                  static_cast<int32_t>(std::lround(y)));
}

void SurfaceControlNativeSetScale(JNIEnv*, jclass, jlong transaction,
                                  jlong control, jfloat x, jfloat y) {
  ASurfaceTransaction_setScale(SurfaceTransaction(transaction),
                               SurfaceControl(control), x, y);
}

void SurfaceControlNativeSetLayer(JNIEnv*, jclass, jlong transaction,
                                  jlong control, jint layer) {
  ASurfaceTransaction_setZOrder(SurfaceTransaction(transaction),
                                SurfaceControl(control), layer);
}

void SurfaceControlNativeSetRelativeLayer(JNIEnv*, jclass, jlong transaction,
                                          jlong control, jlong relative_to,
                                          jint layer) {
  darwin_art_android_surface_transaction_set_relative_layer(
      reinterpret_cast<void*>(transaction), reinterpret_cast<void*>(control),
      reinterpret_cast<void*>(relative_to), layer);
}

void SurfaceControlNativeReparent(JNIEnv*, jclass, jlong transaction,
                                  jlong control, jlong parent) {
  ASurfaceTransaction_reparent(SurfaceTransaction(transaction),
                               SurfaceControl(control), SurfaceControl(parent));
}

void SurfaceControlNativeSetAlpha(JNIEnv*, jclass, jlong transaction,
                                  jlong control, jfloat alpha) {
  ASurfaceTransaction_setBufferAlpha(SurfaceTransaction(transaction),
                                     SurfaceControl(control), alpha);
}

void SurfaceControlNativeSetMatrix(JNIEnv*, jclass, jlong transaction,
                                   jlong control, jfloat dsdx, jfloat,
                                   jfloat, jfloat dtdy) {
  ASurfaceTransaction_setScale(SurfaceTransaction(transaction),
                               SurfaceControl(control), dsdx, dtdy);
}

void SurfaceControlNativeSetWindowCrop(JNIEnv*, jclass, jlong transaction,
                                       jlong control, jint left, jint top,
                                       jint right, jint bottom) {
  const ARect crop{left, top, right, bottom};
  ASurfaceTransaction_setCrop(SurfaceTransaction(transaction),
                              SurfaceControl(control), crop);
}

void SurfaceControlNativeSetDestinationFrame(JNIEnv*, jclass, jlong transaction,
                                            jlong control, jint left, jint top,
                                            jint right, jint bottom) {
  if (left >= right || top >= bottom) return;  // Empty frame: keep buffer size.
  (void)darwin_art_android_surface_transaction_set_destination_frame(
      SurfaceTransaction(transaction), SurfaceControl(control),
      ARect{left, top, right, bottom});
}

void SurfaceControlNativeSetBufferTransform(JNIEnv*, jclass, jlong transaction,
                                            jlong control, jint transform) {
  ASurfaceTransaction_setBufferTransform(SurfaceTransaction(transaction),
                                         SurfaceControl(control), transform);
}

void SurfaceControlNativeApplyTransaction(JNIEnv*, jclass, jlong transaction,
                                          jboolean, jboolean) {
  ASurfaceTransaction_apply(
      reinterpret_cast<ASurfaceTransaction*>(transaction));
}

void SurfaceControlNativeSetExtendedRangeBrightness(JNIEnv*, jclass,
                                                    jlong transaction,
                                                    jlong control,
                                                    jfloat current_ratio,
                                                    jfloat desired_ratio) {
  // Standard-dynamic-range windows require no headroom transform. The Metal
  // composer consumes these values when HDR layer state is plumbed; the
  // optional hint must not abort an otherwise valid HWUI transaction.
  ASurfaceTransaction_setExtendedRangeBrightness(
      SurfaceTransaction(transaction), SurfaceControl(control), current_ratio,
      desired_ratio);
}

void SurfaceControlNativeSetColor(JNIEnv* env, jclass, jlong transaction,
                                  jlong control, jfloatArray color) {
  jfloat values[3]{0.0f, 0.0f, 0.0f};
  if (color != nullptr && env->GetArrayLength(color) >= 3) {
    env->GetFloatArrayRegion(color, 0, 3, values);
  }
  ASurfaceTransaction_setColor(
      SurfaceTransaction(transaction), SurfaceControl(control), values[0],
      values[1], values[2], 1.0f, ADATASPACE_UNKNOWN);
}

void SurfaceControlNativeSetDesiredHdrHeadroom(JNIEnv*, jclass,
                                               jlong transaction,
                                               jlong control, jfloat ratio) {
  // The SDR Metal swapchain has a fixed headroom of 1.0. Keep the framework
  // transaction valid; HDR negotiation belongs to the display backend.
  ASurfaceTransaction_setDesiredHdrHeadroom(
      SurfaceTransaction(transaction), SurfaceControl(control), ratio);
}

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint count) {
  if (env == nullptr) return false;
  jclass clazz = env->FindClass(class_name);
  if (clazz == nullptr) return false;
  const jint result = env->RegisterNatives(clazz, methods, count);
  env->DeleteLocalRef(clazz);
  return result == JNI_OK;
}

}  // namespace

namespace darwin_art::window {

bool RegisterSurfaceControlNatives(JNIEnv* env) {
  if (!ResetSurfaceTransactionSubmission()) return false;
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeCreate"),
       const_cast<char*>(
           "(Landroid/view/SurfaceSession;Ljava/lang/String;IIIIJ"
           "Landroid/os/Parcel;)J"),
       reinterpret_cast<void*>(&SurfaceControlNativeCreate)},
      {const_cast<char*>("nativeGetHandle"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&SurfaceControlNativeGetHandle)},
      {const_cast<char*>("nativeCopyFromSurfaceControl"),
       const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&SurfaceControlNativeCopy)},
      {const_cast<char*>("nativeReadFromParcel"),
       const_cast<char*>("(Landroid/os/Parcel;)J"),
       reinterpret_cast<void*>(&SurfaceControlNativeReadFromParcel)},
      {const_cast<char*>("nativeWriteToParcel"),
       const_cast<char*>("(JLandroid/os/Parcel;)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeWriteToParcel)},
      {const_cast<char*>("nativeDisconnect"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeDisconnect)},
      {const_cast<char*>("nativeGetNativeSurfaceControlFinalizer"),
       const_cast<char*>("()J"),
       reinterpret_cast<void*>(&SurfaceControlNativeGetFinalizer)},
      {const_cast<char*>("nativeGetNativeTransactionFinalizer"),
       const_cast<char*>("()J"),
       reinterpret_cast<void*>(&SurfaceTransactionNativeGetFinalizer)},
      {const_cast<char*>("nativeCreateTransaction"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&SurfaceControlNativeCreateTransaction)},
      {const_cast<char*>("nativeApplyTransaction"),
       const_cast<char*>("(JZZ)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeApplyTransaction)},
      {const_cast<char*>("nativeSetTransformHint"), const_cast<char*>("(JI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetTransformHint)},
      {const_cast<char*>("nativeSetFrameRateCategory"),
       const_cast<char*>("(JJIZ)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetFrameRateCategory)},
      {const_cast<char*>("nativeClearTransaction"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeClearTransaction)},
      {const_cast<char*>("nativeMergeTransaction"), const_cast<char*>("(JJ)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeMergeTransaction)},
      {const_cast<char*>("nativeSetAnimationTransaction"),
       const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop1)},
      {const_cast<char*>("nativeSetEarlyWakeupStart"),
       const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop1)},
      {const_cast<char*>("nativeSetEarlyWakeupEnd"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop1)},
      {const_cast<char*>("nativeSetFlags"), const_cast<char*>("(JJII)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetFlags)},
      {const_cast<char*>("nativeSetPosition"), const_cast<char*>("(JJFF)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetPosition)},
      {const_cast<char*>("nativeSetScale"), const_cast<char*>("(JJFF)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetScale)},
      {const_cast<char*>("nativeSetLayer"), const_cast<char*>("(JJI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetLayer)},
      {const_cast<char*>("nativeSetLayerStack"), const_cast<char*>("(JJI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetRelativeLayer"),
       const_cast<char*>("(JJJI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetRelativeLayer)},
      {const_cast<char*>("nativeReparent"), const_cast<char*>("(JJJ)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeReparent)},
      {const_cast<char*>("nativeSetAlpha"), const_cast<char*>("(JJF)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetAlpha)},
      {const_cast<char*>("nativeSetColor"), const_cast<char*>("(JJ[F)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetColor)},
      {const_cast<char*>("nativeSetDesiredHdrHeadroom"),
       const_cast<char*>("(JJF)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetDesiredHdrHeadroom)},
      {const_cast<char*>("nativeSetMatrix"), const_cast<char*>("(JJFFFF)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetMatrix)},
      {const_cast<char*>("nativeSetWindowCrop"),
       const_cast<char*>("(JJIIII)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetWindowCrop)},
      {const_cast<char*>("nativeSetCrop"), const_cast<char*>("(JJFFFF)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetDestinationFrame"),
       const_cast<char*>("(JJIIII)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetDestinationFrame)},
      {const_cast<char*>("nativeSetCornerRadius"), const_cast<char*>("(JJF)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetShadowRadius"), const_cast<char*>("(JJF)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetColorSpaceAgnostic"),
       const_cast<char*>("(JJZ)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetBufferTransform"),
       const_cast<char*>("(JJI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetBufferTransform)},
      {const_cast<char*>("nativeSetDamageRegion"),
       const_cast<char*>("(JJLandroid/graphics/Region;)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetTransparentRegionHint"),
       const_cast<char*>("(JJLandroid/graphics/Region;)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetTransparentRegionHint)},
      {const_cast<char*>("nativeSetDataSpace"), const_cast<char*>("(JJI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetFrameTimelineVsync"),
       const_cast<char*>("(JJ)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetDesiredPresentTimeNanos"),
       const_cast<char*>("(JJ)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetFixedTransformHint"),
       const_cast<char*>("(JJI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetFrameRate"), const_cast<char*>("(JJFII)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetFrameRateSelectionPriority"),
       const_cast<char*>("(JJI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetFrameRateSelectionStrategy"),
       const_cast<char*>("(JJI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetDimmingEnabled"),
       const_cast<char*>("(JJZ)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetExtendedRangeBrightness"),
       const_cast<char*>("(JJFF)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeSetExtendedRangeBrightness)},
      {const_cast<char*>("nativeSetTrustedOverlay"),
       const_cast<char*>("(JJI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
      {const_cast<char*>("nativeSetDropInputMode"),
       const_cast<char*>("(JJI)V"),
       reinterpret_cast<void*>(&SurfaceControlNativeTransactionNoop2)},
  };
  return Register(env, "android/view/SurfaceControl", methods,
                  static_cast<jint>(std::size(methods)));
}

}  // namespace darwin_art::window
