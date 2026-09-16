package dev.darwinart.system;
public final class DarwinSystemServer {
    private static native String nativeResolvePackage(String name);
    public static android.os.Binder createServiceDirectory() {
        if (!"installed-record".equals(nativeResolvePackage("example.package"))) {
            throw new AssertionError("resolver not registered");
        }
        return new android.os.Binder();
    }
}
