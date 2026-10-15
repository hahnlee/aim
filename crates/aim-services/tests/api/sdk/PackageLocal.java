// Compile-only nested SDK interface; the derived image supplies its implementation.
package dev.aim.server;
public final class PackageLocal {
    public interface SigningOwner {
        void add(android.content.pm.SigningDetails oldDetails, android.content.pm.SigningDetails newDetails);
        void remove(android.content.pm.SigningDetails oldDetails);
        void clear();
    }
    public interface SdkDataOwner {
        void reconcile(String volumeUuid, String packageName, java.util.List<String> subDirNames,
            int userId, int appId, int previousAppId, String seInfo, int flags) throws java.io.IOException;
    }
}
