package dev.darwinart.runtime.pm;

import android.content.pm.ApplicationInfo;
import android.content.pm.ServiceInfo;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;

/** Manifest-declared service projection owned by installed package state. */
public final class InstalledServiceInfo {
    private InstalledServiceInfo() {}

    public static ServiceInfo service(String packageName, String record, String serviceName) {
        InstalledPackageRecord installed = InstalledPackageRecord.fromRecord(packageName, record);
        if (installed == null || serviceName == null || serviceName.isEmpty()) return null;

        String processName = null;
        boolean isolatedProcess = false;
        boolean enabled = true;
        boolean exported = false;
        String permission = null;
        String declarations = installed.legacyManifestHint("services");
        if (declarations != null && !declarations.equals("none")) {
            for (String declaration : declarations.split(",")) {
                String[] fields = declaration.split(">", -1);
                if (fields.length >= 2 && serviceName.equals(fields[0])) {
                    processName = fields[1];
                    // Older manifest records had only name>process (or
                    // name>process>isolated). Keep those records usable with
                    // Android's ordinary defaults.
                    if (fields.length == 3) {
                        isolatedProcess = parseBoolean(fields[2], "service isolatedProcess");
                    } else if (fields.length == 6) {
                        isolatedProcess = parseBoolean(fields[2], "service isolatedProcess");
                        permission = decodePermission(fields[3]);
                        exported = parseBoolean(fields[4], "service exported");
                        enabled = parseBoolean(fields[5], "service enabled");
                    } else if (fields.length != 2) {
                        throw new IllegalArgumentException("Invalid service declaration");
                    }
                    break;
                }
            }
        }
        if (processName == null || processName.isEmpty()) return null;

        ApplicationInfo application = InstalledApplicationInfo.fromRecord(packageName, record);
        ServiceInfo info = new ServiceInfo();
        info.packageName = packageName;
        info.name = serviceName;
        info.processName = processName;
        info.applicationInfo = application;
        info.enabled = enabled;
        info.exported = exported;
        info.permission = permission;
        if (isolatedProcess) info.flags |= ServiceInfo.FLAG_ISOLATED_PROCESS;
        return info;
    }

    private static boolean parseBoolean(String value, String field) {
        if ("1".equals(value)) return true;
        if ("0".equals(value)) return false;
        throw new IllegalArgumentException("Invalid " + field + " field");
    }

    private static String decodePermission(String value) {
        if ("none".equals(value)) return null;
        if ((value.length() & 1) != 0) {
            throw new IllegalArgumentException("Invalid service permission encoding");
        }
        byte[] bytes = new byte[value.length() / 2];
        for (int index = 0; index < bytes.length; index++) {
            int high = Character.digit(value.charAt(index * 2), 16);
            int low = Character.digit(value.charAt(index * 2 + 1), 16);
            if (high < 0 || low < 0) {
                throw new IllegalArgumentException("Invalid service permission encoding");
            }
            bytes[index] = (byte) ((high << 4) | low);
        }
        try {
            return StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(bytes))
                    .toString();
        } catch (CharacterCodingException error) {
            throw new IllegalArgumentException("Invalid service permission encoding", error);
        }
    }
}
