package dev.aim.server;

import android.content.ComponentName;
import android.content.Intent;
import android.os.Binder;
import android.os.Parcel;
import android.os.Process;

/** ResolveIntentHelper's exact final Intent, using the original Parcelable. */
public final class PackageLaunchBridge extends IPackageLaunchBridge.Stub {
    @Override
    public byte[] buildLaunchIntent(String category, String targetPackage, ComponentName component) {
        if (Binder.getCallingUid() != Process.SYSTEM_UID)
            throw new SecurityException("launch intent serialization requires system UID");
        Intent intent = new Intent("android.intent.action.MAIN");
        intent.addCategory(category);
        intent.setFlags(0x10000000); // FLAG_ACTIVITY_NEW_TASK at the pinned version.
        if (component != null) {
            intent.setPackage(null);
            intent.setClassName(component.getPackageName(), component.getClassName());
        } else {
            intent.setPackage(targetPackage);
        }
        // ACTION_MAIN carries neither a type nor data, so resolveTypeIfNeeded
        // is null, exactly as in the original ResolveIntentHelper request.
        Parcel parcel = Parcel.obtain();
        try { intent.writeToParcel(parcel, 0); return parcel.marshall(); }
        finally { parcel.recycle(); }
    }
}
