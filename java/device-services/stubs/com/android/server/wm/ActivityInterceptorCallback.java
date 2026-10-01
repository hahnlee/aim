// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.wm;

import android.app.ActivityOptions;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.pm.ResolveInfo;

public interface ActivityInterceptorCallback {
    int PRODUCT_ORDERED_ID = 5;

    ActivityInterceptResult onInterceptActivityLaunch(ActivityInterceptorInfo info);

    final class ActivityInterceptorInfo {
        public ActivityInterceptorInfo(Builder builder) { throw new RuntimeException("stub"); }
        public int getUserId() { throw new RuntimeException("stub"); }
        public Intent getIntent() { throw new RuntimeException("stub"); }
        public String getCallingPackage() { throw new RuntimeException("stub"); }
        public ActivityOptions getCheckedOptions() { throw new RuntimeException("stub"); }
        public ActivityInfo getActivityInfo() { throw new RuntimeException("stub"); }

        public static final class Builder {
            public Builder(int callingUid, int callingPid, int realCallingUid, int realCallingPid, int userId, Intent intent, ResolveInfo rInfo, ActivityInfo aInfo) { throw new RuntimeException("stub"); }
        }
    }

    final class ActivityInterceptResult {
        public ActivityInterceptResult(Intent intent, ActivityOptions activityOptions) { throw new RuntimeException("stub"); }
    }
}
