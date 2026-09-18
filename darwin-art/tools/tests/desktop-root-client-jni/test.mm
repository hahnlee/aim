#import <AppKit/AppKit.h>
#import <CoreFoundation/CoreFoundation.h>
#include "runtime/framework/wm/desktop_root_client_jni.h"
#include "runtime/framework/wm/desktop_foreground_authority_jni.h"
#include "compat/window/desktop_root_target.h"
#include <cassert>
#include <cstdio>
#include <string>
#include <optional>
#include <unistd.h>

using namespace darwin_art::framework::wm;
int main(int argc, char** argv) {
  assert(argc == 3 && [NSThread isMainThread]);
  @autoreleasepool {
    JavaVM* vm = nullptr;
    JNIEnv* env = nullptr;
    std::string classpath = std::string("-Djava.class.path=") + argv[1];
    JavaVMOption option{classpath.data(), nullptr};
    JavaVMInitArgs args{JNI_VERSION_1_6, 1, &option, JNI_FALSE};
    assert(JNI_CreateJavaVM(&vm, reinterpret_cast<void**>(&env), &args) == JNI_OK);
    assert(RegisterDesktopForegroundAuthority(env));
    jclass authority_test = env->FindClass("dev/darwinart/runtime/wm/DesktopForegroundAuthorityTest");
    assert(authority_test != nullptr && !env->ExceptionCheck());
    jmethodID authority_run = env->GetStaticMethodID(authority_test, "run", "(I)V");
    assert(authority_run != nullptr && !env->ExceptionCheck());
    env->CallStaticVoidMethod(authority_test, authority_run, static_cast<jint>(getpid()));
    assert(!env->ExceptionCheck());
    env->DeleteLocalRef(authority_test);
    NSWindow* window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,100,100)
        styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
    window.releasedWhenClosed = NO;
    auto events = darwin_art::window::DesktopRootEvents::Create(window);
    auto target = darwin_art::window::DesktopRootTarget::Create(events, window);
    assert(target != nullptr && target->Publish());
    assert(RegisterDesktopRootClient(env));
    jclass channel_type = env->FindClass("android/view/InputChannel");
    assert(channel_type != nullptr);
    jmethodID channel_init = env->GetMethodID(channel_type, "<init>", "()V");
    assert(channel_init != nullptr);
    jobject input_channel = env->NewObject(channel_type, channel_init);
    assert(input_channel != nullptr);
    assert(EnsureDesktopRootClient(env, input_channel));
    jclass client_type = env->FindClass("dev/darwinart/runtime/wm/DesktopRootClient");
    jfieldID captured_target = env->GetStaticFieldID(client_type, "capturedTarget", "J");
    assert(captured_target != nullptr && !env->ExceptionCheck());
    const jlong original_handle = env->GetStaticLongField(client_type, captured_target);
    auto original_root = RetainDesktopRootClientTarget(original_handle);
    assert(original_root == events);
    assert(RetainDesktopRootClientTarget(0) == nullptr);
    assert(RetainDesktopRootClientTarget(-1) == nullptr);
    env->DeleteLocalRef(client_type);
    const std::string mode = argv[2];
    const bool cancel_before_bind = mode == "cancel";
    const int expected_facts = cancel_before_bind || mode == "reject" ? 0 : 1;
    if (mode == "reject") {
      assert(target->Close());
      assert(RetainDesktopRootClientTarget(original_handle) == nullptr);
      assert(original_root->Snapshot().closed);
    }
    if (mode == "attach") assert(vm->DetachCurrentThread() == JNI_OK);
    if (!cancel_before_bind) {
      for (int i = 0; i < 5; ++i)
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, false);
    }
    if (mode == "attach")
      assert(vm->AttachCurrentThread(reinterpret_cast<void**>(&env), nullptr) == JNI_OK);
    jclass type = env->FindClass("dev/darwinart/runtime/wm/DesktopRootClient");
    jfieldID facts = env->GetStaticFieldID(type, "facts", "I");
    assert(env->GetStaticIntField(type, facts) == expected_facts);
    jfieldID rejections = env->GetStaticFieldID(type, "rejections", "I");
    assert(env->GetStaticIntField(type, rejections) == (mode == "reject" ? 1 : 0));
    if (mode == "pending") {
      jclass error = env->FindClass("java/lang/IllegalStateException");
      env->ThrowNew(error, "original caller exception");
      jthrowable original = env->ExceptionOccurred();
      assert(events->Notify(DARWIN_ART_DESKTOP_ROOT_RESIGNED));
      assert(env->ExceptionCheck());
      jthrowable retained = env->ExceptionOccurred();
      env->ExceptionClear();
      assert(env->IsSameObject(original, retained));
      env->DeleteLocalRef(retained);
      env->DeleteLocalRef(original);
      env->DeleteLocalRef(error);
    }
    std::optional<darwin_art::window::DesktopRootEvents::DeferredClose> deferred;
    if (mode == "deferred") deferred.emplace(events->PrepareClose());
    assert(CloseDesktopRootClientAdmission(env));
    assert(RetainDesktopRootClientTarget(original_handle) == nullptr);
    assert(!PollDesktopRootClientQuiesced(env)); // queued exact cleanup still owned
    for (int i = 0; i < 20 && !PollDesktopRootClientQuiesced(env); ++i)
      CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, false);
    if (deferred) {
      assert(!PollDesktopRootClientQuiesced(env));
      assert(deferred->Deliver());
      deferred.reset();
    }
    assert(PollDesktopRootClientQuiesced(env));
    assert(env->GetStaticIntField(type, facts) == expected_facts);
    assert(!EnsureDesktopRootClient(env, input_channel));
    env->DeleteLocalRef(input_channel);
    env->DeleteLocalRef(channel_type);
    env->DeleteLocalRef(type);
    assert(ClearDesktopRootClientReferences(env));
    (void)target->Close();
    [window close];
    target.reset();
    events.reset();
    assert(vm->DestroyJavaVM() == JNI_OK);
    std::puts("Actual JVM desktop root: initial fact/canceled bind/quiescence/global cleanup PASS");
  }
}
