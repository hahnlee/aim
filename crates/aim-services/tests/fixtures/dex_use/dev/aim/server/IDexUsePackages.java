package dev.aim.server;
public interface IDexUsePackages {
 Object getSnapshot() throws android.os.RemoteException;
 String filterPackageName(long version,String name,int uid,int user) throws android.os.RemoteException;
 boolean shouldFilter(long version,String name,int uid,int user) throws android.os.RemoteException;
 default long getArtOwnerGeneration(){throw new AssertionError();}
 default void attachArtOwner(long generation,IArtDeletion owner){throw new AssertionError();}
 default void checkArtOwner(long generation){throw new AssertionError();}
 boolean crossUserSuspensions() throws android.os.RemoteException;
}
