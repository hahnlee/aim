package dev.aim.server;
public final class InstallerRecoveryPresentation extends IInstallerRecoveryPresentation.Stub {
    public InstallerRecoveryPresentation(android.content.Context context) { java.util.Objects.requireNonNull(context); }
    @Override public byte[] decodeIcon(byte[] png) {
        Bridge.enforceSystemUid(); java.util.Objects.requireNonNull(png);
        android.graphics.Bitmap bitmap = android.graphics.BitmapFactory.decodeByteArray(png, 0, png.length);
        if (bitmap == null) return null; // BitmapFactory's source-defined decode failure.
        android.os.Parcel parcel = android.os.Parcel.obtain();
        try {
            boolean previous = parcel.pushAllowFds(false);
            try { parcel.writeParcelable(bitmap, 0); }
            finally { parcel.restoreAllowFds(previous); }
            return parcel.marshall();
        } finally { parcel.recycle(); bitmap.recycle(); }
    }
}
