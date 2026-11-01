package dev.aim.server;
import com.android.server.pm.PackageManagerLocal;
public final class PackageLocal implements PackageManagerLocal {
 public void closeEpoch() {}
 public static int uid,user;
 public FilteredSnapshot withFilteredSnapshot(int uid,android.os.UserHandle user){PackageLocal.uid=uid;PackageLocal.user=user.getIdentifier();return new FilteredSnapshot(){};}
}
