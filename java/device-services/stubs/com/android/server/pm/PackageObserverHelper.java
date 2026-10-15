// Compile-only pinned original PackageObserverHelper; used unchanged at runtime.
package com.android.server.pm;
class PackageObserverHelper {
    PackageObserverHelper() { throw new RuntimeException("stub"); }
    public void addObserver(android.content.pm.PackageManagerInternal.PackageListObserver observer) { throw new RuntimeException("stub"); }
    public void removeObserver(android.content.pm.PackageManagerInternal.PackageListObserver observer) { throw new RuntimeException("stub"); }
    public void notifyAdded(String name,int uid) { throw new RuntimeException("stub"); }
    public void notifyChanged(String name,int uid) { throw new RuntimeException("stub"); }
    public void notifyRemoved(String name,int uid) { throw new RuntimeException("stub"); }
}
