package dev.aim.server;
public final class PackageScanLease implements AutoCloseable {
    public static final java.util.Map<IPackageScanSnapshot, java.util.Map<String,PackageUserStateData>> states = new java.util.IdentityHashMap<>();
    public static int closed;
    private final IPackageScanSnapshot endpoint;
    public PackageScanLease(IPackageScanSnapshot endpoint) throws android.os.RemoteException { this.endpoint = endpoint; }
    public long getVersion() { return 1; }
    public PackageUserStateData getUserState(String name, boolean factory, int user) throws java.io.IOException {
        if (user != 0) return null;
        return states.get(endpoint).get(name);
    }
    public PackageCode getCode(String name, boolean factory) throws java.io.IOException { return new PackageCode(name); }
    public void close() throws android.os.RemoteException { closed++; endpoint.close(); }
}
