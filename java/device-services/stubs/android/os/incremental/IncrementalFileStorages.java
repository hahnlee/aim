// Compile-only image ABI, not included at runtime.
package android.os.incremental;
public class IncrementalFileStorages {
 private IncrementalFileStorages(java.io.File stage,java.io.File inherited,IncrementalManager manager,android.content.pm.DataLoaderParams params){throw new RuntimeException("stub");}
    public void cleanUpAndMarkComplete(){throw new RuntimeException("stub");}
    public static IncrementalFileStorages initialize(android.content.Context context,java.io.File stage,java.io.File inherited,
        android.content.pm.DataLoaderParams params,android.content.pm.IDataLoaderStatusListener status,
        android.os.incremental.StorageHealthCheckParams health,android.os.incremental.IStorageHealthListener listener,
        java.util.List<android.content.pm.InstallationFileParcel> added,android.os.incremental.PerUidReadTimeouts[] timeouts,
        android.content.pm.IPackageLoadingProgressCallback progress)throws java.io.IOException{throw new RuntimeException("stub");}
    public void startLoading(android.content.pm.DataLoaderParams params,android.content.pm.IDataLoaderStatusListener status,
        android.os.incremental.StorageHealthCheckParams health,android.os.incremental.IStorageHealthListener listener,
        android.os.incremental.PerUidReadTimeouts[] timeouts)throws java.io.IOException{throw new RuntimeException("stub");}
}
