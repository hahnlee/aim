// Compile-only original image type; no runtime implementation.
package com.android.server.pm;
public class PackageList implements android.content.pm.PackageManagerInternal.PackageListObserver, AutoCloseable {
    public PackageList(java.util.List<String> names, android.content.pm.PackageManagerInternal.PackageListObserver observer) { throw new RuntimeException("stub"); }
    public void onPackageAdded(String name, int uid) { throw new RuntimeException("stub"); }
    public void onPackageChanged(String name, int uid) { throw new RuntimeException("stub"); }
    public void onPackageRemoved(String name, int uid) { throw new RuntimeException("stub"); }
    public void close() throws Exception { throw new RuntimeException("stub"); }
    public java.util.List<String> getPackageNames() { throw new RuntimeException("stub"); }
}
