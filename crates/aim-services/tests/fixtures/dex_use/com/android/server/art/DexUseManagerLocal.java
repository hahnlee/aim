package com.android.server.art;
public final class DexUseManagerLocal {
 public static Injector injector; public static int calls,uid,user;public static java.util.Map<String,String> map;public static boolean fail;
 public DexUseManagerLocal(Injector value){injector=value;if(value.getPackageManagerLocal()!=com.android.server.LocalManagerRegistry.getManager(com.android.server.pm.PackageManagerLocal.class))throw new AssertionError("package local must register first");}
 public void notifyDexContainersLoaded(com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot,String name,java.util.Map<String,String> contexts){
  calls++;uid=injector.getCallingUid();user=injector.getCallingUserHandle().getIdentifier();map=contexts;
  if(uid!=dev.aim.server.PackageLocal.uid||user!=dev.aim.server.PackageLocal.user)throw new AssertionError("identity differs from snapshot");
  if(fail)throw new IllegalArgumentException("original owner error");
 }
 public static class Injector {
  // DexUseManagerLocal.Injector checks GlobalInjector's ART registration before PMS.
  Injector(android.content.Context context){ArtModuleServiceInitializer.getArtModuleServiceManager();getPackageManagerLocal();}
  public com.android.server.pm.PackageManagerLocal getPackageManagerLocal(){throw new AssertionError();}
  public int getCallingUid(){throw new AssertionError();}
  public android.os.UserHandle getCallingUserHandle(){throw new AssertionError();}
 }
}
