// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app.role;

import android.os.UserHandle;

import java.util.List;
import java.util.concurrent.Executor;

public final class RoleManager {
    public static final String ROLE_DEVICE_POLICY_MANAGEMENT = "android.app.role.DEVICE_POLICY_MANAGEMENT";
    public static final String ROLE_BROWSER = "android.app.role.BROWSER";
    public RoleManager(android.content.Context context, IRoleManager service) { throw new RuntimeException("stub"); }
    public List<String> getRoleHoldersAsUser(String roleName, UserHandle user) { throw new RuntimeException("stub"); }
    public void addOnRoleHoldersChangedListenerAsUser(Executor executor, OnRoleHoldersChangedListener listener, UserHandle user) { throw new RuntimeException("stub"); }
    public static String ROLE_DIALER;
    public void addRoleHolderAsUser(String roleName,String packageName,int flags,UserHandle user,Executor executor,java.util.function.Consumer<Boolean> callback) { throw new RuntimeException("stub"); }
}
