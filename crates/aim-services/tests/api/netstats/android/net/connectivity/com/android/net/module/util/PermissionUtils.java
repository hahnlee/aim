// Host JVM test double; production links the retained image classes.
package android.net.connectivity.com.android.net.module.util;
public class PermissionUtils {
 public static void enforcePackageNameMatchesUid(android.content.Context c,int uid,String pkg){
  if(uid!=android.os.Binder.uid)throw new AssertionError("caller identity");
  if(!pkg.equals("app.pkg"))throw new SecurityException("package UID mismatch");
 }
}
