// Compile-only pinned image API; runtime reads the original fields.
package android.app;
public class AppOpsManager {
    public static final int OP_UNARCHIVAL_CONFIRMATION = 146;
    public int checkOp(int op,int uid,String name) { throw new RuntimeException("stub"); }
    public static final int OP_AUTO_REVOKE_PERMISSIONS_IF_UNUSED = 97;
    public static final int MODE_IGNORED = 1;
    public int checkOpNoThrow(int op, int uid, String packageName) { throw new RuntimeException("stub"); }
 public static int OP_SYSTEM_EXEMPT_FROM_SUSPENSION=124,OP_ARCHIVE_ICON_OVERLAY=145;
 public void checkPackage(int uid,String name){throw new RuntimeException("stub");}
 AppOpsManager(android.content.Context context,com.android.internal.app.IAppOpsService service){throw new RuntimeException("stub");}
}
