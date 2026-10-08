// Compile-only pinned image API.
package android.content.pm.parsing.result;
public interface ParseInput {
    interface Callback {
        boolean isChangeEnabled(long changeId, String packageName, int targetSdkVersion);
    }
}
