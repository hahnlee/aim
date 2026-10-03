/* Ported from android-16.0.0_r1 Settings reader loops.
 * Copyright (C) The Android Open Source Project, Apache License 2.0.
 * Private Settings methods are not invoked: parsers and setting owners are original. */
package com.android.server.pm;

import com.android.modules.utils.TypedXmlPullParser;
import com.android.server.pm.permission.LegacyPermissionState;
import dev.aim.server.PackageLegacyPermissions;
import java.io.File;
import java.nio.file.Files;
import java.util.LinkedHashMap;
import java.util.Map;

public final class LegacyRestoreOracle {
    private final Map<String, PackageSetting> active = new LinkedHashMap<>();
    private final Map<String, PackageSetting> factory = new LinkedHashMap<>();
    private final Map<String, SharedUserSetting> shared = new LinkedHashMap<>();
    private final Map<Integer, SettingBase> visible = new LinkedHashMap<>();
    private final int[] users = {10, 0};

    private LegacyRestoreOracle() {
        seed("android.uid.system", 1000); seed("android.uid.phone", 1001);
        seed("android.uid.log", 1007); seed("android.uid.nfc", 1027);
        seed("android.uid.bluetooth", 1002); seed("android.uid.shell", 2000);
        seed("android.uid.se", 1031); seed("android.uid.networkstack", 1073);
        seed("android.uid.uwb", 1083); seed("android.uid.vendor.fixture", 2901);
    }
    private void seed(String name, int id) {
        var owner = new SharedUserSetting(name, 0, 0);
        owner.mAppId = id; shared.put(name, owner); visible.put(id, owner);
    }
    public static void verify(File directory) throws Exception {
        for (int i = 0; i < 4; i++) {
            var root = new File(directory, "legacy-restore-" + i);
            var oracle = new LegacyRestoreOracle();
            try (var input = new java.io.FileInputStream(new File(root, "system/packages.xml"))) {
                var parser = android.util.Xml.resolvePullParser(input);
                while (parser.next() != 2) {}
                oracle.readSettings(parser);
            }
            for (var entry : oracle.active.entrySet()) oracle.compare(root, "active-" + entry.getKey(), entry.getValue());
            for (var entry : oracle.factory.entrySet()) oracle.compare(root, "factory-" + entry.getKey(), entry.getValue());
            for (var entry : oracle.shared.entrySet()) {
                var owner = entry.getValue();
                oracle.compareBytes(root, "shared-" + entry.getKey(), owner.mAppId, owner.getLegacyPermissionState());
            }
            long actual = java.util.Arrays.stream(root.listFiles()).filter(f -> f.getName().endsWith(".input")).count();
            if (actual != oracle.active.size() + oracle.factory.size() + oracle.shared.size()) throw new AssertionError("restored owner inventory differs");
        }
    }
    private void compare(File root, String stem, PackageSetting owner) throws Exception {
        compareBytes(root, stem, owner.getAppId(), owner.getLegacyPermissionState());
        boolean expected = Boolean.parseBoolean(new String(Files.readAllBytes(new File(root, stem + ".fixed").toPath()), java.nio.charset.StandardCharsets.UTF_8));
        if (expected != owner.isInstallPermissionsFixed()) throw new AssertionError("install permissions fixed differs: " + stem);
    }
    private void compareBytes(File root, String stem, int id, LegacyPermissionState state) throws Exception {
        byte[] expected = Files.readAllBytes(new File(root, stem + ".input").toPath());
        byte[] actual = PackageLegacyPermissions.capture(id, users, state);
        if (!java.util.Arrays.equals(expected, actual)) throw new AssertionError("Settings permission restoration differs: " + stem);
    }
    private void readSettings(TypedXmlPullParser parser) throws Exception {
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
            if (event == 3 || event == 4) continue;
            switch (parser.getName()) {
                case "package": readPackage(parser, false); break;
                case "updated-package": readPackage(parser, true); break;
                case "shared-user": readShared(parser); break;
                case "version": case "renamed-package": case "verifier":
                case "last-platform-version": case "database-version":
                case "preferred-packages": case "read-external-storage": break;
                default: skip(parser);
            }
        }
    }
    private void readPackage(TypedXmlPullParser parser, boolean disabled) throws Exception {
        String name = parser.getAttributeValue(null, "name");
        var owner = new PackageSetting(name, null, new File(parser.getAttributeValue(null, "codePath")), 0, 0, new java.util.UUID(0, 0));
        int id = parser.getAttributeInt(null, "userId", 0);
        if (id <= 0) {
            id = parser.getAttributeInt(null, "sharedUserId", 0);
            owner.setSharedUserAppId(id);
        } else if (!disabled) visible.put(id, owner);
        owner.setAppId(id);
        (disabled ? factory : active).put(name, owner);
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
            if (event == 3 || event == 4) continue;
            String tag = parser.getName();
            if ("perms".equals(tag)) {
                SettingBase target = owner.hasSharedUser() ? visible.get(id) : owner;
                if (target != null) {
                    readInstall(parser, target.getLegacyPermissionState());
                    if (!disabled) owner.setInstallPermissionsFixed(true);
                }
            } else if (!disabled && ("proper-signing-keyset".equals(tag) || "signing-keyset".equals(tag)
                    || "upgrade-keyset".equals(tag) || "defined-keyset".equals(tag))) {
                // Attribute-only and obsolete branches do not consume their subtree.
            } else skip(parser);
        }
    }
    private void readShared(TypedXmlPullParser parser) throws Exception {
        String name = parser.getAttributeValue(null, "name");
        int id = parser.getAttributeInt(null, "userId");
        if (!shared.containsKey(name)) seed(name, id);
        var owner = shared.get(name);
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
            if (event == 3 || event == 4) continue;
            if ("perms".equals(parser.getName())) readInstall(parser, owner.getLegacyPermissionState());
            else skip(parser);
        }
    }
    private void readInstall(TypedXmlPullParser parser, LegacyPermissionState state) throws Exception {
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
            if (event == 3 || event == 4) continue;
            if ("item".equals(parser.getName())) {
                String name = parser.getAttributeValue(null, "name");
                boolean granted = parser.getAttributeBoolean(null, "granted", true);
                int flags = parser.getAttributeIntHex(null, "flags", 0);
                for (int user : users) state.putPermissionState(new LegacyPermissionState.PermissionState(name, false, granted, flags), user);
            } else skip(parser);
        }
    }
    private static void skip(TypedXmlPullParser parser) throws Exception {
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {}
    }
}
