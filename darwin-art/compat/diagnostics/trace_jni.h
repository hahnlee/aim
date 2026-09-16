#pragma once
#include <jni.h>
namespace android { int register_android_os_Trace(JNIEnv*); }
namespace tracing_perfetto { void registerWithPerfetto(bool); }
