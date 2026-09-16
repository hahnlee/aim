#pragma once

#include <jni.h>

namespace darwin_art::framework::wm {

// Constructs the original AOSP launch/resume ClientTransaction. Android owns
// activity construction, ContextImpl, lifecycle callbacks and ViewRoot setup.
bool ScheduleActivityLaunch(JNIEnv* env, jobject application_binder,
                            jstring package_name, jstring installed_record);

// Schedules a manifest-resolved Intent through the same original AOSP client
// transaction path used for the initial launcher Activity.
bool ScheduleResolvedActivityLaunch(JNIEnv* env, jobject application_binder,
                                    jobject previous_activity_token,
                                    jobject activity_token, jobject intent,
                                    jobject activity_info);

bool RegisterActivityLaunchScheduler(JNIEnv* env, jclass endpoint);

}  // namespace darwin_art::framework::wm
