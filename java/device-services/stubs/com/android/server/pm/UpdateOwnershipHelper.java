// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm;
public class UpdateOwnershipHelper {
    public android.util.ArraySet<String> readUpdateOwnerDenyList(PackageSetting pkg) { throw new RuntimeException("stub"); }
    public void addToUpdateOwnerDenyList(String provider, android.util.ArraySet<String> contents) { throw new RuntimeException("stub"); }
    public void removeUpdateOwnerDenyList(String provider) { throw new RuntimeException("stub"); }
    public boolean isUpdateOwnershipDenylisted(String name) { throw new RuntimeException("stub"); }
    public boolean isUpdateOwnershipDenyListProvider(String name) { throw new RuntimeException("stub"); }
}
