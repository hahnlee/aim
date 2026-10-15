package dev.aim.server;

import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Bundle;
import android.os.Parcel;
import android.os.UserHandle;

/** Original Parcelable serialization for the native package monitor owner. */
public final class NativePackageMonitorPayload {
    private NativePackageMonitorPayload() {}
    public static byte[] serialize(String action, String packageName, Bundle extras, int userId) {
        Intent intent = new Intent(action,
                packageName == null ? null : Uri.fromParts("package", packageName, null));
        if (extras != null) intent.putExtras(extras);
        int uid = intent.getIntExtra(Intent.EXTRA_UID, -1);
        if (uid >= 0 && UserHandle.getUserId(uid) != userId)
            intent.putExtra(Intent.EXTRA_UID, UserHandle.getUid(userId, UserHandle.getAppId(uid)));
        intent.putExtra(Intent.EXTRA_USER_HANDLE, userId);
        Bundle result = new Bundle();
        result.putParcelable(PackageManager.EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT, intent);
        Parcel parcel = Parcel.obtain();
        try { result.writeToParcel(parcel, 0); return parcel.marshall(); }
        finally { parcel.recycle(); }
    }
}
