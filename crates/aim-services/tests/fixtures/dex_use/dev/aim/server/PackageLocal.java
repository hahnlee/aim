package dev.aim.server;
import com.android.server.pm.PackageManagerLocal;
public final class PackageLocal implements PackageManagerLocal {
 public interface Sdk {void run(String v,String n,java.util.List<String> d,int u,int a,int p,String s,int f);}
 public interface SigningOwner {void add(android.content.pm.SigningDetails a,android.content.pm.SigningDetails b);void remove(android.content.pm.SigningDetails a);void clear();}
 public PackageLocal(PackageSnapshots.Store store,Sdk sdk,SigningOwner signing){}
 public static int uid,user;
 public FilteredSnapshot withFilteredSnapshot(int uid,android.os.UserHandle user){PackageLocal.uid=uid;PackageLocal.user=user.getIdentifier();return new FilteredSnapshot(){};}
}
