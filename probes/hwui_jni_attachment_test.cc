#include "darwin_hwui_jni_attachment.h"
#include <atomic>
#include <cassert>
#include <thread>

namespace {
pthread_key_t art_key;
std::atomic<int> attaches{0}, detaches{0}, late_exits{0};
JNIEnv dummy_env{};

jint getEnv(JavaVM*, void** output, jint) {
    *output = pthread_getspecific(art_key) ? &dummy_env : nullptr;
    return *output ? JNI_OK : JNI_EDETACHED;
}
jint attach(JavaVM*, JNIEnv** output, void*) {
    assert(pthread_setspecific(art_key, reinterpret_cast<void*>(1)) == 0);
    *output = &dummy_env;
    ++attaches;
    return JNI_OK;
}
jint detach(JavaVM*) {
    assert(pthread_getspecific(art_key));
    assert(pthread_setspecific(art_key, nullptr) == 0);
    ++detaches;
    return JNI_OK;
}
void artExit(void*) { ++late_exits; }
}

int main() {
    assert(pthread_key_create(&art_key, artExit) == 0);
    JNIInvokeInterface table{};
    table.GetEnv = getEnv;
    table.AttachCurrentThreadAsDaemon = attach;
    table.DetachCurrentThread = detach;
    JavaVM vm{&table};
    // Every callback in one worker shares attachment ownership; C++ TLS must
    // clear ART's pthread slot before ART's thread-exit diagnostic callback.
    for (int i = 0; i < 100; ++i) {
        std::thread worker([&] {
            assert(aim::hwui::requireEnv(&vm, "hwuiTask") == &dummy_env);
            assert(aim::hwui::requireEnv(&vm) == &dummy_env);
        });
        worker.join();
    }
    assert(attaches == 100 && detaches == 100 && late_exits == 0);
    // A Java-owned/externally-attached thread stays owned by its caller.
    std::thread borrowed([&] {
        JNIEnv* env;
        assert(attach(&vm, &env, nullptr) == JNI_OK);
        { aim::hwui::JniAttachment scope; assert(scope.get(&vm) == env); }
        assert(detaches == 100);
        assert(detach(&vm) == JNI_OK);
    });
    borrowed.join();
    // Explicit detach is not repeated at native worker destruction.
    std::thread explicit_detach([&] {
        aim::hwui::requireEnv(&vm);
        assert(detach(&vm) == JNI_OK);
    });
    explicit_detach.join();
    assert(attaches == 102 && detaches == 102 && late_exits == 0);
    assert(pthread_key_delete(art_key) == 0);
    std::puts("hwui-jni-attachment: PASS owned=100 borrowed=1 explicit-detach=1 ART-TLS-exit=0");
}
