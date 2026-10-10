// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public class UserManagerService extends android.os.IUserManager.Stub {
    public int[] getUserIds(){throw new RuntimeException("stub");}
    UserManagerService(android.content.Context context) { throw new RuntimeException("stub"); }
    UserManagerService(android.content.Context context, PackageManagerService pm,
            UserDataPreparer preparer, Object packagesLock) { throw new RuntimeException("stub"); }
    void systemReady() { throw new RuntimeException("stub"); }
    void reconcileUsers(String volumeUuid) { throw new RuntimeException("stub"); }
    public static UserManagerService getInstance() { throw new RuntimeException("stub"); }
    public java.util.List<android.content.pm.UserInfo> getUsers(boolean excludePartial,
            boolean excludeDying, boolean excludePreCreated) { throw new RuntimeException("stub"); }
    public boolean hasUserRestriction(String key, int userId) { throw new RuntimeException("stub"); }
    public java.util.List<android.content.pm.UserInfo> getUsers(boolean excludeDying) { throw new RuntimeException("stub"); }
    public android.content.pm.UserInfo getProfileParent(int userId) { throw new RuntimeException("stub"); }
    public int getCrossProfileIntentFilterAccessControl(int source, int target) { throw new RuntimeException("stub"); }
 public boolean isSameProfileGroup(int first,int second){throw new RuntimeException("stub");}
    public android.content.pm.UserInfo createUserWithThrow(String name,String type,int flags){throw new RuntimeException("stub");}
    public android.content.pm.UserInfo preCreateUserWithThrow(String type){throw new RuntimeException("stub");}
    public android.content.pm.UserInfo createProfileForUserWithThrow(String name,String type,int flags,int user,String[] packages){throw new RuntimeException("stub");}
    public android.content.pm.UserInfo createRestrictedProfileWithThrow(String name,int parent){throw new RuntimeException("stub");}
}
