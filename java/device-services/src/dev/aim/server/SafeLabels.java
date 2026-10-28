package dev.aim.server;

import android.text.TextUtils;

/** PackageItemInfo.loadSafeLabel's non-hot presentation step, android-16.0.0_r1. */
public final class SafeLabels extends ISafeLabels.Stub {
    @Override
    public String makeSafeLabel(String label) {
        Bridge.enforceSystemUid();
        return TextUtils.makeSafeForPresentation(label, 1000, 1000f,
                TextUtils.SAFE_STRING_FLAG_TRIM | TextUtils.SAFE_STRING_FLAG_FIRST_LINE)
                .toString();
    }
}
