package com.android.server.art;
import dev.aim.server.PackageSnapshots;
public final class DexUseTest {
 public static void main(String[] args){
  dev.aim.server.IDexUsePackages packages=new dev.aim.server.IDexUsePackages(){
   public Object getSnapshot(){return this;}
   public String filterPackageName(long v,String n,int uid,int user){return n;}
   public boolean shouldFilter(long v,String n,int uid,int user){return false;}
   public boolean crossUserSuspensions(){return true;}
  };
  try{new AppHostDexUse(null,packages);throw new AssertionError("accepted missing ART initializer");}catch(NullPointerException expected){}
  ArtModuleServiceInitializer.setArtModuleServiceManager(new android.os.ArtModuleServiceManager());
  AppHostDexUse bridge=new AppHostDexUse(null,packages);
  if(com.android.server.LocalManagerRegistry.getManager(DexUseManagerLocal.class)==null)throw new AssertionError("unregistered owner");
  try{new AppHostDexUse(null,packages);throw new AssertionError("duplicate owner");}catch(IllegalStateException expected){}
  String[] paths={"/data/user/1/a.b/test.dex"};String[] contexts={"PCL[]"};
  android.os.Binder.uid=10100;
  try{bridge.notifyDexLoad("a.b",paths,contexts,"arm64",199001);throw new AssertionError();}catch(SecurityException expected){}
  if(PackageSnapshots.Store.refreshes!=2)throw new AssertionError("untrusted caller reached captures");
  android.os.Binder.uid=1000;
  try{bridge.notifyDexLoad("a.b",paths,new String[0],null,199001);throw new AssertionError();}catch(IllegalArgumentException expected){}
  bridge.notifyDexLoad("a.b",paths,contexts,"x86",199001);
  if(DexUseManagerLocal.calls!=0)throw new AssertionError("invalid ISA reached owner");
  bridge.notifyDexLoad("a.b",paths,contexts,"arm64",199001);
  if(DexUseManagerLocal.uid!=199001||DexUseManagerLocal.user!=1||!DexUseManagerLocal.map.equals(java.util.Map.of(paths[0],contexts[0])))throw new AssertionError("forwarded report differs");
  try{DexUseManagerLocal.injector.getCallingUid();throw new AssertionError("identity leaked");}catch(NullPointerException expected){}
  // Null and empty maps reach the original unchanged: it owns their validation.
  bridge.notifyDexLoad("a.b",null,null,null,10100);
  if(DexUseManagerLocal.map!=null)throw new AssertionError();
  bridge.notifyDexLoad("a.b",new String[0],new String[0],null,10100);
  if(!DexUseManagerLocal.map.isEmpty())throw new AssertionError();
  DexUseManagerLocal.fail=true;
  try{bridge.notifyDexLoad("a.b",paths,contexts,null,10100);throw new AssertionError();}catch(IllegalArgumentException expected){if(!expected.getMessage().equals("original owner error"))throw expected;}
  try{DexUseManagerLocal.injector.getCallingUid();throw new AssertionError("failed call leaked identity");}catch(NullPointerException expected){}
 }
}
