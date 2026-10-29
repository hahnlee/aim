package dev.aim.server;
public interface IDexUsePackages {
 Object getSnapshot() throws android.os.RemoteException;
 String filterPackageName(long version,String name,int uid,int user) throws android.os.RemoteException;
 boolean shouldFilter(long version,String name,int uid,int user) throws android.os.RemoteException;
 boolean crossUserSuspensions() throws android.os.RemoteException;
}
