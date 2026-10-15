// Compile-only original API. No stub is included in the runtime artifact.
package android.app;
import android.content.pm.*;
import android.os.UserHandle;
public class ApplicationPackageManager extends PackageManager {
    protected ApplicationPackageManager(ContextImpl context, IPackageManager packages) { throw new RuntimeException("stub"); }
    public static void invalidateGetPackagesForUidCache() { throw new RuntimeException("stub"); }
    public ApplicationInfo getApplicationInfo(String packageName, int flags) throws NameNotFoundException { throw new RuntimeException("stub"); }
    public PackageInfo getPackageInfoAsUser(String packageName, int flags, int userId) throws NameNotFoundException { throw new RuntimeException("stub"); }
    public int getPermissionFlags(String permName, String packageName, UserHandle user) { throw new RuntimeException("stub"); }
    public boolean hasSystemFeature(String featureName) { throw new RuntimeException("stub"); }
    public java.util.List<PermissionGroupInfo> getAllPermissionGroups(int flags) { throw new RuntimeException("stub"); }
    public java.util.List<PermissionInfo> queryPermissionsByGroup(String permissionGroup, int flags) throws NameNotFoundException { throw new RuntimeException("stub"); }
    public PermissionInfo getPermissionInfo(String permissionName, int flags) throws NameNotFoundException { throw new RuntimeException("stub"); }
    public PackageInstaller getPackageInstaller() { throw new RuntimeException("stub"); }
    public String getPermissionControllerPackageName() { throw new RuntimeException("stub"); }
    public int checkPermission(String permission, String packageName) { throw new RuntimeException("stub"); }
}
