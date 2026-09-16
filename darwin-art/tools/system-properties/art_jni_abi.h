#pragma once
#include "core_jni_helpers.h"

// ART executes Android boot classes on Darwin. Only layoutlib's host ABI adds
// JNIEnv/jclass to CriticalNative calls. Select ART's actual critical ABI without
// pretending Darwin libc provides unrelated __ANDROID__ pthread extensions.
#undef CRITICAL_JNI_PARAMS
#undef CRITICAL_JNI_PARAMS_COMMA
#define CRITICAL_JNI_PARAMS
#define CRITICAL_JNI_PARAMS_COMMA

// The translation unit supplies its own tag after this forced include.
#undef LOG_TAG
