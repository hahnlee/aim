// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public abstract class UserManagerService extends android.os.IUserManager.Stub {
    UserManagerService(android.content.Context context) { throw new RuntimeException("stub"); }
    public static UserManagerService getInstance() { throw new RuntimeException("stub"); }
    public java.util.List<android.content.pm.UserInfo> getUsers(boolean excludePartial,
            boolean excludeDying, boolean excludePreCreated) { throw new RuntimeException("stub"); }
    public boolean hasUserRestriction(String key, int userId) { throw new RuntimeException("stub"); }
}
