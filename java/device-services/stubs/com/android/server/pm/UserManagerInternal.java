// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm;

public abstract class UserManagerInternal {
    public interface UserVisibilityListener {
        void onUserVisibilityChanged(int userId, boolean visible);
    }

    public UserManagerInternal() { throw new RuntimeException("stub"); }
    public abstract void addUserVisibilityListener(UserVisibilityListener listener);
    public abstract int[] getUserIds();
    public abstract java.util.List<android.content.pm.UserInfo> getUsers(boolean excludePartial, boolean excludeDying, boolean excludePreCreated);
    public abstract android.content.pm.UserInfo[] getUserInfos();
    public abstract android.content.pm.UserInfo getUserInfo(int userId);
    public abstract int getProfileParentId(int userId);
    public abstract boolean exists(int userId);
    public abstract boolean hasUserRestriction(String restriction, int userId);
    public abstract boolean isUserRunning(int userId);
    public abstract boolean isUserUnlockingOrUnlocked(int userId);
    public abstract android.content.pm.UserProperties getUserProperties(int userId);
 public abstract int[] getProfileIds(int user,boolean enabled);

}
