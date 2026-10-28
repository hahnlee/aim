package android.content;
public class Context {
 public final java.util.Set<String> grants = new java.util.HashSet<>();
 public int checkPermission(String permission,int pid,int uid){return grants.contains(uid+":"+permission)?0:-1;}
 public android.content.pm.PackageManager getPackageManager(){throw new AssertionError();}
}
