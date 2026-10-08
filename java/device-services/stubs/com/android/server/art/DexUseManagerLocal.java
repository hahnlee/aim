// Compile-only pinned image API; never packaged as runtime implementation.
package com.android.server.art;

public class DexUseManagerLocal {
    private DexUseManagerLocal(android.content.Context context) { throw new RuntimeException("stub"); }
    public void notifyDexContainersLoaded(com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot,
            String name, java.util.Map<String, String> contexts) { throw new RuntimeException("stub"); }
    public void systemReady() { throw new RuntimeException("stub"); }
}
