// Compile-only pinned original ART API; never added to the derived image.
package com.android.server.art;
public final class ArtManagerLocal {
    public ArtManagerLocal(android.content.Context context) { throw new RuntimeException("stub"); }
    public interface DexoptDoneCallback { void onDexoptDone(com.android.server.art.model.DexoptResult result); }
    public void addDexoptDoneCallback(boolean onlyUpdates, java.util.concurrent.Executor executor, DexoptDoneCallback callback) { throw new RuntimeException("stub"); }
    public int scheduleBackgroundDexoptJob() { throw new RuntimeException("stub"); }
    public void onApexStaged(String[] names) { throw new RuntimeException("stub"); }
    public void dump(java.io.PrintWriter writer, com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot) { throw new RuntimeException("stub"); }
    public void dumpPackage(java.io.PrintWriter writer, com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot, String packageName) { throw new RuntimeException("stub"); }
    public com.android.server.art.model.DexoptResult dexoptPackage(com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot,String name,com.android.server.art.model.DexoptParams params){throw new RuntimeException("stub");}
 public void onBoot(String reason,java.util.concurrent.Executor executor,java.util.function.Consumer<com.android.server.art.model.OperationProgress> callback){throw new RuntimeException("stub");}
 public void clearAppProfiles(com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot,String name){throw new RuntimeException("stub");}
 public com.android.server.art.model.DeleteResult deleteDexoptArtifacts(com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot,String name){throw new RuntimeException("stub");}
 public int handleShellCommand(android.os.Binder target,android.os.ParcelFileDescriptor input,
         android.os.ParcelFileDescriptor output,android.os.ParcelFileDescriptor error,String[] arguments) {
     throw new RuntimeException("stub");
 }
}
