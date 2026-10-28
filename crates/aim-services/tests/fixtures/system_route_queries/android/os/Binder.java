package android.os;
public class Binder {
 public static int uid=1000;
 public static boolean cleared;
 public static int getCallingUid(){return uid;}
 public static long clearCallingIdentity(){if(cleared)throw new AssertionError(); cleared=true;return 42;}
 public static void restoreCallingIdentity(long t){if(t!=42||!cleared)throw new AssertionError();cleared=false;}
}
