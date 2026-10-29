package dev.aim.server;
public final class PackageSnapshots {
 public interface Owner {String getFilteredPackageName(long v,String name,int uid,int user);boolean shouldFilter(long v,com.android.server.pm.pkg.PackageState state,int uid,int user);}
 public interface Source {Object capture() throws android.os.RemoteException;}
 public static final class Store {
  public static int refreshes; private final Source source;
  public Store(Source source,Owner owner,boolean cross){this.source=source;}
  public void refresh() throws android.os.RemoteException,java.io.IOException {if(source.capture()==null)throw new java.io.IOException("missing capture");refreshes++;}
 }
}
