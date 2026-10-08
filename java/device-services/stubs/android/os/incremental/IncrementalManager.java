// Compile-only pinned original API; runtime uses the original manager.
package android.os.incremental;
public final class IncrementalManager {
    public boolean registerLoadingProgressCallback(String path, android.content.pm.IPackageLoadingProgressCallback callback) { throw new RuntimeException("stub"); }
    public static boolean isIncrementalPath(String path){throw new RuntimeException("stub");}
 public IncrementalManager(IIncrementalService service){throw new RuntimeException("stub");}
}
