// Compile-only pinned image API; checked by the device-services build node.
package android.os;
public class UserManager {
    public static int getMaxSupportedUsers(){throw new RuntimeException("stub");}
    public UserManager(android.content.Context context, IUserManager service) { throw new RuntimeException("stub"); }
    public static final String DISALLOW_INSTALL_UNKNOWN_SOURCES = "no_install_unknown_sources";
    public static final String DISALLOW_INSTALL_UNKNOWN_SOURCES_GLOBALLY = "no_install_unknown_sources_globally";
    public static final String DISALLOW_INSTALL_APPS = "no_install_apps";
    public static final String DISALLOW_DEBUGGING_FEATURES = "no_debugging_features";
    public java.util.List<android.content.pm.UserInfo> getUsers(){throw new RuntimeException("stub");}
 public static boolean isHeadlessSystemUserMode(){throw new RuntimeException("stub");}
 public static final String DISALLOW_UNINSTALL_APPS="no_uninstall_apps";
 public boolean hasUserRestriction(String restriction,UserHandle user){throw new RuntimeException("stub");}
}
