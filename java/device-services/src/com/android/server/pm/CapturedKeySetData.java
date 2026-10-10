package com.android.server.pm;

import dev.aim.server.PackageSettingData;

/** Detached original keyset objects rebuilt from captured owner inputs. */
public final class CapturedKeySetData {
    private CapturedKeySetData() {}
    public static PackageKeySetData from(PackageSettingData.KeySetData data) {
        var keys = new PackageKeySetData();
        populate(data, keys);
        return keys;
    }
    static void populate(PackageSettingData.KeySetData data, PackageKeySetData keys) {
        keys.setProperSigningKeySet(data.properSigningKeySet);
        long[] upgrades = data.getUpgradeKeySets();
        if (upgrades != null) for (long id : upgrades) keys.addUpgradeKeySetById(id);
        for (var alias : data.aliases) keys.addDefinedKeySet(alias.id(), alias.name());
    }
}
