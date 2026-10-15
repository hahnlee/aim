// Compile-only pinned image API; checked by the device-services node.
package com.android.server.pm;
import android.content.Context;
import android.util.SparseArray;
import java.util.List;
public class ProtectedPackages {
    public ProtectedPackages(Context context) { throw new RuntimeException("stub"); }
    public void setDeviceAndProfileOwnerPackages(int deviceOwnerUserId, String deviceOwner, SparseArray<String> profileOwners) { throw new RuntimeException("stub"); }
    public void setOwnerProtectedPackages(int userId, List<String> packageNames) { throw new RuntimeException("stub"); }
    public boolean isPackageStateProtected(int userId, String packageName) { throw new RuntimeException("stub"); }
    public boolean isPackageDataProtected(int userId, String packageName) { throw new RuntimeException("stub"); }
 public String getDeviceOwnerOrProfileOwnerPackage(int user){throw new RuntimeException("stub");}
}
