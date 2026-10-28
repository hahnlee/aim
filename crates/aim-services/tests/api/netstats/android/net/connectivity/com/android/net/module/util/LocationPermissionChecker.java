// Host JVM test double; production links the retained image classes.
package android.net.connectivity.com.android.net.module.util;
public class LocationPermissionChecker {
 public static boolean allowed;
 public LocationPermissionChecker(android.content.Context c){}
 public boolean checkCallersLocationPermission(String pkg,String feature,int uid,boolean coarse,String message){
  if(!pkg.equals("app.pkg")||uid!=android.os.Binder.uid||feature!=null||coarse||message!=null)throw new AssertionError("location args");
  return allowed;
 }
}
