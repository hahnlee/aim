// Compile-only original lifecycle owner.
package com.android.server.pm;
public class PackageMonitorCallbackHelper {
    public PackageMonitorCallbackHelper() { throw new RuntimeException("stub"); }
    public void registerPackageMonitorCallback(android.os.IRemoteCallback callback, int userId, int uid) { throw new RuntimeException("stub"); }
    public void unregisterPackageMonitorCallback(android.os.IRemoteCallback callback) { throw new RuntimeException("stub"); }
    public void onUserRemoved(int userId) { throw new RuntimeException("stub"); }
    public void notifyPackageChanged(String name, boolean dontKill, java.util.ArrayList<String> components, int uid, String reason, int[] users, int[] instant, android.util.SparseArray<int[]> allowLists, android.os.Handler handler) { throw new RuntimeException("stub"); }
    public void notifyPackageAddedForNewUsers(String name, int appId, int[] users, int[] instant, boolean archived, int loader, android.util.SparseArray<int[]> allowLists, android.os.Handler handler) { throw new RuntimeException("stub"); }
    public void notifyPackageMonitor(String action, String name, android.os.Bundle extras, int[] users, int[] instant, android.util.SparseArray<int[]> allowLists, android.os.Handler handler, java.util.function.BiFunction<Integer,android.os.Bundle,android.os.Bundle> filter) { throw new RuntimeException("stub"); }
}
