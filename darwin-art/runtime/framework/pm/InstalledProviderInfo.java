package dev.darwinart.runtime.pm;

import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.content.pm.ProviderInfo;
import java.util.HashSet;

/** Manifest-declared provider projection owned by installed package state. */
public final class InstalledProviderInfo {
    private InstalledProviderInfo() {}

    public static ProviderInfo provider(String packageName, String record,
            String providerName, long flags) {
        InstalledPackageRecord installed = InstalledPackageRecord.fromRecord(packageName, record);
        if (installed == null || providerName == null || providerName.isEmpty()) return null;
        String declarations = installed.legacyManifestHint("providers");
        if (declarations == null || declarations.equals("none")) return null;
        if (declarations.isEmpty()) throw new IllegalArgumentException("Empty provider declarations");

        HashSet<String> names = new HashSet<>();
        ProviderInfo result = null;
        for (String declaration : declarations.split(";", -1)) {
            String[] fields = declaration.split(">", -1);
            if (fields.length != 5) throw new IllegalArgumentException("Invalid provider declaration");
            String name = InstalledManifestMetadata.decodeUtf8Hex(fields[0], "provider name");
            String authority = InstalledManifestMetadata.decodeUtf8Hex(
                    fields[1], "provider authority");
            if (name.isEmpty() || authority.isEmpty() || name.indexOf('\0') >= 0
                    || authority.indexOf('\0') >= 0 || !names.add(name)) {
                throw new IllegalArgumentException("Invalid or duplicate provider identity");
            }
            int initOrder = parseUnsignedHex(fields[2]);
            if (!fields[3].equals("0") && !fields[3].equals("1")) {
                throw new IllegalArgumentException("Invalid provider grantUriPermissions");
            }
            if (!name.equals(providerName)) continue;

            ApplicationInfo application = InstalledApplicationInfo.fromRecord(packageName, record);
            ProviderInfo info = new ProviderInfo();
            info.packageName = packageName;
            info.name = name;
            info.processName = packageName;
            info.authority = authority;
            info.initOrder = initOrder;
            info.grantUriPermissions = fields[3].equals("1");
            info.applicationInfo = application;
            if ((flags & PackageManager.GET_META_DATA) != 0) {
                info.metaData = InstalledManifestMetadata.fromEncoded(installed, fields[4]);
            }
            result = info;
        }
        return result;
    }

    private static int parseUnsignedHex(String value) {
        if (value.isEmpty() || value.length() > 8) {
            throw new IllegalArgumentException("Invalid provider initOrder");
        }
        for (int index = 0; index < value.length(); index++) {
            if (Character.digit(value.charAt(index), 16) < 0) {
                throw new IllegalArgumentException("Invalid provider initOrder");
            }
        }
        return (int) Long.parseLong(value, 16);
    }
}
