#pragma once

#include <jni.h>

namespace darwin_art::framework::wm {

// Constructs the original AOSP launch/resume ClientTransaction. Android owns
// activity construction, ContextImpl, lifecycle callbacks and ViewRoot setup.
bool ScheduleActivityLaunch(JNIEnv* env, jobject application_binder,
                            jstring package_name, jint uid);

// Schedules a manifest-resolved Intent through the same original AOSP client
// transaction path used for the initial launcher Activity.
// {current, override} come from the ActivityTask task geometry owner.
bool ScheduleResolvedActivityLaunch(JNIEnv* env, jobject application_binder,
                                    jobject previous_activity_token,
                                    jobject activity_token, jobject intent,
                                    jobject activity_info, jobject current,
                                    jobject override);

bool RegisterActivityLaunchScheduler(JNIEnv* env, jclass endpoint);

}  // namespace darwin_art::framework::wm
