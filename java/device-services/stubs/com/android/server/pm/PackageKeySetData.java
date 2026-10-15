// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public class PackageKeySetData {
    PackageKeySetData() {}
    PackageKeySetData(PackageKeySetData original) {}
    protected void setProperSigningKeySet(long id) { throw new RuntimeException("stub"); }
    protected void addUpgradeKeySetById(long id) { throw new RuntimeException("stub"); }
    protected void addDefinedKeySet(long id, String alias) { throw new RuntimeException("stub"); }
    protected void removeAllUpgradeKeySets() { throw new RuntimeException("stub"); }
    protected void removeAllDefinedKeySets() { throw new RuntimeException("stub"); }

    protected long[] getUpgradeKeySets() { throw new RuntimeException("stub"); }
    protected android.util.ArrayMap<String, Long> getAliases() { throw new RuntimeException("stub"); }
    protected long getProperSigningKeySet() { throw new RuntimeException("stub"); }
}
