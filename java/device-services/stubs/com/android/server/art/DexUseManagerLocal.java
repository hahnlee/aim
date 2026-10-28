// Compile-only pinned image API; never packaged as runtime implementation.
package com.android.server.art;

public class DexUseManagerLocal {
    public DexUseManagerLocal(Injector injector) { throw new RuntimeException("stub"); }
    public static class Injector {
        Injector(android.content.Context context) { throw new RuntimeException("stub"); }
        public com.android.server.pm.PackageManagerLocal getPackageManagerLocal() { throw new RuntimeException("stub"); }
        public int getCallingUid() { throw new RuntimeException("stub"); }
        public android.os.UserHandle getCallingUserHandle() { throw new RuntimeException("stub"); }
        public String getFilename() { throw new RuntimeException("stub"); }
        public java.util.concurrent.ScheduledExecutorService createScheduledExecutor() { throw new RuntimeException("stub"); }
    }
    private DexUseManagerLocal(android.content.Context context) { throw new RuntimeException("stub"); }
    public void notifyDexContainersLoaded(com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot,
            String name, java.util.Map<String, String> contexts) { throw new RuntimeException("stub"); }
    public void systemReady() { throw new RuntimeException("stub"); }
    public long getPackageLastUsedAtMs(String name) { throw new RuntimeException("stub"); }
}
