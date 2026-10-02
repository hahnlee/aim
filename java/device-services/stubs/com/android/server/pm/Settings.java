// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public final class Settings {
    public Settings(java.util.Map<String, PackageSetting> packages) { throw new RuntimeException("stub"); }
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
