// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm.verify.domain;
public class DomainVerificationService extends com.android.server.SystemService implements DomainVerificationManagerInternal {
    public DomainVerificationService(android.content.Context context,
            com.android.server.SystemConfig config, com.android.server.compat.PlatformCompat compat) {
        super(context); throw new RuntimeException("stub");
    }
    @Override public void onStart() { throw new RuntimeException("stub"); }
    public void addPackage(com.android.server.pm.pkg.PackageStateInternal setting, android.content.pm.verify.domain.DomainSet preVerified) { throw new RuntimeException("stub"); }
    public void migrateState(com.android.server.pm.pkg.PackageStateInternal oldSetting, com.android.server.pm.pkg.PackageStateInternal newSetting, android.content.pm.verify.domain.DomainSet preVerified) { throw new RuntimeException("stub"); }
    public void readSettings(com.android.server.pm.Computer snapshot, com.android.modules.utils.TypedXmlPullParser parser) throws java.io.IOException { throw new RuntimeException("stub"); }
    public void writeSettings(com.android.server.pm.Computer snapshot, com.android.modules.utils.TypedXmlSerializer serializer, boolean signatures, int user) throws java.io.IOException { throw new RuntimeException("stub"); }
    public void setConnection(DomainVerificationManagerInternal.Connection connection) { throw new RuntimeException("stub"); }
    public android.content.pm.verify.domain.DomainVerificationInfo getDomainVerificationInfo(String name) { throw new RuntimeException("stub"); }
    public android.content.pm.verify.domain.DomainVerificationUserState getDomainVerificationUserState(String name, int user) { throw new RuntimeException("stub"); }
    public int setDomainVerificationStatus(java.util.UUID id, java.util.Set<String> domains, int state) throws android.content.pm.PackageManager.NameNotFoundException { throw new RuntimeException("stub"); }
    public void setDomainVerificationLinkHandlingAllowedInternal(String name, boolean allowed, int user) throws android.content.pm.PackageManager.NameNotFoundException { throw new RuntimeException("stub"); }
    public void clearPackage(String name) { throw new RuntimeException("stub"); }
    public void clearPackageForUser(String name, int user) { throw new RuntimeException("stub"); }
    public void clearUser(int user) { throw new RuntimeException("stub"); }
    public void readLegacySettings(com.android.modules.utils.TypedXmlPullParser parser) throws java.io.IOException { throw new RuntimeException("stub"); }
    public java.util.UUID generateNewId() { throw new RuntimeException("stub"); }
}
