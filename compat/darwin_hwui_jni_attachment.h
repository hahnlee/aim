#pragma once

#include <jni.h>
#include <cstdio>
#include <cstdlib>
#include <pthread.h>

namespace aim::hwui {

// Borrowed JNIEnv never transfers ownership. Only the attaching native worker
// may detach. Darwin C++ TLS destructors precede pthread-key destructors, so
// ART still has its normal thread state here, before ThreadExitCallback.
class JniAttachment final {
public:
    JniAttachment() = default;
    JniAttachment(const JniAttachment&) = delete;
    JniAttachment& operator=(const JniAttachment&) = delete;

    ~JniAttachment() {
        if (mOwnedVm == nullptr) return;
        JNIEnv* env = nullptr;
        const jint state = mOwnedVm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6);
        if (state == JNI_EDETACHED) return; // Explicit detach already ran.
        if (state != JNI_OK || mOwnedVm->DetachCurrentThread() != JNI_OK) std::abort();
        if (std::getenv("AIM_DEBUG_JNI_ATTACH")) {
            std::fprintf(stderr, "ART HWUI JNI detach tid=%p result=0 owned=1\n",
                         reinterpret_cast<void*>(pthread_self()));
        }
    }

    JNIEnv* get(JavaVM* vm, const char* name = nullptr) {
        if (vm == nullptr || (mOwnedVm != nullptr && mOwnedVm != vm)) std::abort();
        JNIEnv* env = nullptr;
        const jint state = vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6);
        if (state == JNI_OK) return env;
        if (state != JNI_EDETACHED) std::abort();
        JavaVMAttachArgs args{JNI_VERSION_1_6, const_cast<char*>(name), nullptr};
        if (vm->AttachCurrentThreadAsDaemon(&env, &args) != JNI_OK) std::abort();
        mOwnedVm = vm;
        if (std::getenv("AIM_DEBUG_JNI_ATTACH")) {
            std::fprintf(stderr, "ART HWUI JNI attach name=%s tid=%p owned=1 state=JNI_EDETACHED attach=JNI_OK\n",
                         name ? name : "callback", reinterpret_cast<void*>(pthread_self()));
        }
        return env;
    }

private:
    JavaVM* mOwnedVm = nullptr;
};

inline JNIEnv* requireEnv(JavaVM* vm, const char* name = nullptr) {
    static thread_local JniAttachment attachment;
    return attachment.get(vm, name);
}

} // namespace aim::hwui
