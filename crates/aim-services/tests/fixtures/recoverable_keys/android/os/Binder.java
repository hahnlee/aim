// Host-only Android/owner boundary double; production bridge compiled unchanged.
package android.os;
public class Binder {
 public static int uid=1000;
 public static int getCallingUid(){return uid;}
 public static long clearCallingIdentity(){int old=uid;uid=1000;return old;}
 public static void restoreCallingIdentity(long id){uid=(int)id;}
}
