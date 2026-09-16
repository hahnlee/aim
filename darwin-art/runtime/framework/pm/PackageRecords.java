package dev.darwinart.runtime.pm;

/** Installed-record transport endpoint. No metadata interpretation or lifecycle state. */
public final class PackageRecords {
    public interface Source {
        String resolveInstalledPackage(String packageName);
    }
    private PackageRecords() {}
    public static native String nativeResolveInstalledPackage(String packageName);
}
