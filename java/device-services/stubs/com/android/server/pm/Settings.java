// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public final class Settings {
    static void updatePackageSetting(PackageSetting pkg, PackageSetting disabled,
        SharedUserSetting existingShared, SharedUserSetting shared, java.io.File path,
        String legacyLib, String primaryAbi, String secondaryAbi, int flags, int privateFlags,
        UserManagerService users, String[] sdkLibraries, long[] sdkVersions, boolean[] sdkOptional,
        String[] staticLibraries, long[] staticVersions, java.util.Set<String> mimeGroups,
        java.util.UUID domainId, int targetSdk, byte[] restrictHash, boolean dontKill) {
        throw new RuntimeException("stub");
    }
    public Settings(java.util.Map<String, PackageSetting> packages) { throw new RuntimeException("stub"); }
    String addRenamedPackageLPw(String name, String original) { throw new RuntimeException("stub"); }
    String getRenamedPackageLPr(String name) { throw new RuntimeException("stub"); }
    void removeRenamedPackageLPw(String name) { throw new RuntimeException("stub"); }
    public PackageSetting getDisabledSystemPkgLPr(String name) { throw new RuntimeException("stub"); }
    void convertSharedUserSettingsLPw(SharedUserSetting shared) { throw new RuntimeException("stub"); }
    boolean checkAndPruneSharedUserLPw(SharedUserSetting shared, boolean skipCheck) { throw new RuntimeException("stub"); }
    boolean removePackageAndAppIdLPw(String name) { throw new RuntimeException("stub"); }
    boolean registerAppIdLPw(PackageSetting setting, boolean forceNew) throws Exception { throw new RuntimeException("stub"); }
    SharedUserSetting getSharedUserLPw(String name, int flags, int privateFlags, boolean create) throws Exception { throw new RuntimeException("stub"); }
    SharedUserSetting addSharedUserLPw(String name, int uid, int flags, int privateFlags) { throw new RuntimeException("stub"); }
    boolean disableSystemPackageLPw(String name, boolean replaced) { throw new RuntimeException("stub"); }
    void addPackageSettingLPw(PackageSetting setting, SharedUserSetting shared) { throw new RuntimeException("stub"); }
    public SettingBase getSettingLPr(int id) { throw new RuntimeException("stub"); }
    void addInstallerPackageNames(InstallSource source) { throw new RuntimeException("stub"); }
    void readPreferredActivitiesLPw(com.android.modules.utils.TypedXmlPullParser parser, int userId)
        throws java.io.IOException { throw new RuntimeException("stub"); }
    void writePreferredActivitiesLPr(com.android.modules.utils.TypedXmlSerializer serializer, int userId, boolean full)
        throws java.io.IOException { throw new RuntimeException("stub"); }
    void clearPackagePreferredActivities(String packageName, android.util.SparseBooleanArray changed, int userId) { throw new RuntimeException("stub"); }
    static PackageSetting createNewSetting(String name, PackageSetting original,
        PackageSetting disabled, String realName, SharedUserSetting shared,
        java.io.File path, String legacyLib, String primaryAbi, String secondaryAbi,
        long version, int flags, int privateFlags, android.os.UserHandle user,
        boolean allowInstall, boolean instant, boolean virtual, boolean stoppedSystem,
        UserManagerService users, String[] sdkLibraries, long[] sdkVersions,
        boolean[] sdkOptional, String[] staticLibraries, long[] staticVersions,
        java.util.Set<String> mimeGroups, java.util.UUID domainId, int targetSdk, byte[] restrictHash) {
        throw new RuntimeException("stub");
    }
}
