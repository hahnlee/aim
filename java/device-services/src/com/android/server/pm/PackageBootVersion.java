package com.android.server.pm;

/** Current pinned Settings build owner, independent of PMS construction. */
public final class PackageBootVersion {
    public static byte[] capture() {
        var current = new Settings.VersionInfo();
        current.forceCurrent();
        var parcel = android.os.Parcel.obtain();
        try {
            parcel.writeInt(current.sdkVersion);
            parcel.writeInt(current.databaseVersion);
            parcel.writeString(current.buildFingerprint);
            parcel.writeString(current.fingerprint);
            return parcel.marshall();
        } finally { parcel.recycle(); }
    }
    private PackageBootVersion() {}
}
