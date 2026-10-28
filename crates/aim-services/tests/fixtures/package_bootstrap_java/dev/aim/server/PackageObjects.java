package dev.aim.server;
public final class PackageObjects {
    public static com.android.internal.pm.parsing.pkg.PackageImpl fromSnapshot(PackageCode code, long version, String name) {
        return new com.android.internal.pm.parsing.pkg.PackageImpl(name);
    }
}
