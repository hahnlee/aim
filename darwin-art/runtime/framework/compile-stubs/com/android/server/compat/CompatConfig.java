package com.android.server.compat;
import android.content.Context;
import android.content.pm.ApplicationInfo;
import com.android.internal.compat.AndroidBuildClassifier;
/** Compile-only original services.jar API; never a program definition. */
public final class CompatConfig {
    public CompatConfig(AndroidBuildClassifier classifier, Context context) {}
    public native void addChange(CompatChange change);
    public native long[] getDisabledChanges(ApplicationInfo application);
    public native boolean isChangeEnabled(long changeId, ApplicationInfo application);
    public native long[] getLoggableChanges(ApplicationInfo application);
}
