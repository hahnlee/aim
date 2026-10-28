// Host JVM test double; production links the retained image classes.
package android.os;
public class Binder implements IBinder {
 public static int uid=10123, pid=902;
 public static int getCallingUid(){return uid;}
 public static int getCallingPid(){return pid;}
}
