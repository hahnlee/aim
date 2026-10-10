// Compile-only pinned image API.
package android.apex;
public class ApexSessionParams {
    public int sessionId;
    public int[] childSessionIds;
    public boolean isRollback,hasRollbackEnabled;
    public int rollbackId;
    public ApexSessionParams() { throw new RuntimeException("stub"); }
}
