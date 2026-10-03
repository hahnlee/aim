// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm;
public class KeySetManagerService {
    public KeySetManagerService(com.android.server.utils.WatchedArrayMap<String, PackageSetting> packages) { throw new RuntimeException("stub"); }
    void addSigningKeySetToPackageLPw(PackageSetting setting, android.util.ArraySet<java.security.PublicKey> keys) { throw new RuntimeException("stub"); }
    void addDefinedKeySetsToPackageLPw(PackageSetting setting, java.util.Map<String, android.util.ArraySet<java.security.PublicKey>> keys) { throw new RuntimeException("stub"); }
    void addUpgradeKeySetsToPackageLPw(PackageSetting setting, java.util.Set<String> aliases) { throw new RuntimeException("stub"); }
    public void removeAppKeySetDataLPw(String name) { throw new RuntimeException("stub"); }
    void writeKeySetManagerServiceLPr(com.android.modules.utils.TypedXmlSerializer serializer) throws java.io.IOException { throw new RuntimeException("stub"); }
    void readKeySetsLPw(com.android.modules.utils.TypedXmlPullParser parser, android.util.ArrayMap<Long, Integer> refs) throws java.io.IOException { throw new RuntimeException("stub"); }
}
