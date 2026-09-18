#pragma once

#include <jni.h>

namespace darwin_art::framework::wm {

// Native owner for one exact desktop-root key-decision admission.  The owner
// retains the canonical root, authority and Binder wire, but never decides
// input readiness or creates an input route.
bool RegisterRootKeyDecisionClient(JNIEnv* env);
bool CloseRootKeyDecisionAdmission(JNIEnv* env);
bool PollRootKeyDecisionQuiesced(JNIEnv* env);
bool ClearRootKeyDecisionReferences(JNIEnv* env);

}  // namespace darwin_art::framework::wm
