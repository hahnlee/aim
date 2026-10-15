// Compile-only pinned image API; never packaged as runtime implementation.
package android.content;
public class PermissionChecker {public static final int PID_UNKNOWN=-1,PERMISSION_GRANTED=0;
 public static int checkPermissionForPreflight(Context context,String permission,int pid,int uid,String name){throw new RuntimeException("stub");}}
