import com.android.server.pm.permission.LegacyPermissionState;
import dev.aim.server.PackageLegacyPermissions;

/** Actual original LegacyPermissionState owners and production transport codec. */
public final class LegacyPermissionOracle {
    public static void verify(java.io.File directory) throws Exception {
        verifyMigration(directory);
        int[] users = {10, 0, 11};
        var source = new LegacyPermissionState();
        source.setMissing(true, 10);
        source.setMissing(true, 11); source.setMissing(false, 11);
        source.putPermissionState(new LegacyPermissionState.PermissionState("BB", false, true, 0), 10);
        source.putPermissionState(new LegacyPermissionState.PermissionState(null, false, false, Integer.MIN_VALUE), 10);
        source.putPermissionState(new LegacyPermissionState.PermissionState("", true, true, -1), 10);
        source.putPermissionState(new LegacyPermissionState.PermissionState("Aa", false, true, Integer.MAX_VALUE), 10);
        source.putPermissionState(new LegacyPermissionState.PermissionState("BB", true, false, 17), 10);
        source.putPermissionState(new LegacyPermissionState.PermissionState("android.permission.CAMERA", true, true, 0x408030), 0);
        byte[] captured = PackageLegacyPermissions.capture(10042, users, source);
        java.nio.file.Files.write(new java.io.File(directory, "legacy-permissions.original").toPath(), captured);
        byte[] nativeBytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "legacy-permissions.input").toPath());
        if (!java.util.Arrays.equals(captured, nativeBytes)) throw new AssertionError("native legacy permission capture differs");
        var restored = PackageLegacyPermissions.restore(10042, users, nativeBytes);
        if (!restored.isMissing(10) || restored.isMissing(0) || restored.isMissing(11)
            || !restored.getPermissionStates(11).isEmpty() || restored.getPermissionStates(10).size() != 4) throw new AssertionError("permission users/missing state lost");
        for (int user : users) {
            var expected = source.getPermissionStates(user).iterator();
            for (var actual : restored.getPermissionStates(user)) {
                if (!actual.equals(expected.next())) throw new AssertionError("original permission getters differ");
            }
            if (expected.hasNext()) throw new AssertionError("missing permission state");
        }
        try { restored.getPermissionStates(10).clear(); throw new AssertionError("mutable original permission collection"); }
        catch (UnsupportedOperationException expected) {}
        var copied = new LegacyPermissionState(); copied.copyFrom(restored); copied.copyFrom(copied);
        source.reset(); restored.reset(); nativeBytes[0] = 0;
        if (!java.util.Arrays.equals(captured, PackageLegacyPermissions.capture(10042, users, copied))) throw new AssertionError("permission snapshot/copy isolation lost");
        var another = PackageLegacyPermissions.restore(10042, users, captured);
        copied.putPermissionState(new LegacyPermissionState.PermissionState("Aa", true, false, 123), 10);
        copied.setMissing(false, 10);
        if (another.getPermissionState("Aa", 10).getFlags() != Integer.MAX_VALUE || !another.isMissing(10)) throw new AssertionError("original replicas share mutable state");
        for (int user : new int[] {-1, Integer.MIN_VALUE}) {
            try { another.getPermissionStates(user); throw new AssertionError("original accepted negative user"); } catch (IllegalArgumentException expected) {}
            try { another.setMissing(true, user); throw new AssertionError("original accepted negative missing user"); } catch (IllegalArgumentException expected) {}
        }
        rejects(10043, users, captured);
        rejects(10042, new int[] {0, 10, 11}, captured);
        rejects(10042, new int[] {10, 10, 11}, captured);
        rejects(10042, new int[] {}, captured);
        byte[] invalid = captured.clone(); invalid[12] = 2; rejects(10042, users, invalid);
        invalid = java.util.Arrays.copyOf(captured, captured.length + 4); rejects(10042, users, invalid);
        for (int length = 0; length < captured.length; length++) {
            try { rejects(10042, users, java.util.Arrays.copyOf(captured, length)); }
            catch (AssertionError failure) { throw new AssertionError("accepted truncated length " + length, failure); }
        }
    }
    private static void verifyMigration(java.io.File directory) throws Exception {
        for (int i = 0; i < 16; i++) {
            var setting = new com.android.server.pm.PackageSetting("fixture", null,
                new java.io.File("/data/app/fixture"), 0, 0, new java.util.UUID(0, 0));
            var state = setting.getLegacyPermissionState();
            state.putPermissionState(new LegacyPermissionState.PermissionState("seed", false, false, Integer.MAX_VALUE), 0);
            state.setMissing(true, 10);
            try (var input = new java.io.FileInputStream(new java.io.File(directory, "legacy-migration-" + i + ".xml"))) {
                var parser = android.util.Xml.resolvePullParser(input);
                while (parser.next() != 2) {}
                readMigration(parser, state, i % 8 >= 4);
            }
            // Port of Settings.readPermissionsState on the original destination owner.
            state.putPermissionState(new LegacyPermissionState.PermissionState("modern", true, false, Integer.MIN_VALUE), 0);
            state.putPermissionState(new LegacyPermissionState.PermissionState("seed", true, true, 0x408030), 0);
            byte[] expected = java.nio.file.Files.readAllBytes(new java.io.File(directory, "legacy-migration-" + i + ".input").toPath());
            byte[] actual = PackageLegacyPermissions.capture(10042, new int[] {10, 0, 11}, state);
            if (!java.util.Arrays.equals(expected, actual)) throw new AssertionError("legacy migration differs on case " + i);
            java.nio.file.Files.write(new java.io.File(directory, "legacy-migration-" + i + ".original").toPath(), actual);
            if (!state.hasPermissionState(java.util.List.of("seed", "modern")) || state.hasPermissionState(java.util.List.of("absent"))) throw new AssertionError("permission membership differs");
            var copied = new com.android.server.pm.PackageSetting(setting, false);
            state.reset();
            if (!java.util.Arrays.equals(actual, PackageLegacyPermissions.capture(10042, new int[] {10, 0, 11}, copied.getLegacyPermissionState()))) throw new AssertionError("SettingBase copy lost legacy state");
            setting.copySettingBase(copied);
            copied.getLegacyPermissionState().reset();
            if (!java.util.Arrays.equals(actual, PackageLegacyPermissions.capture(10042, new int[] {10, 0, 11}, setting.getLegacyPermissionState()))) throw new AssertionError("copySettingBase shares legacy state");
        }
    }
    // Pinned Settings.readInstallPermissionsLPr / parseLegacyPermissionsLPr.
    private static void readMigration(com.android.modules.utils.TypedXmlPullParser parser,
            LegacyPermissionState state, boolean runtime) throws Exception {
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
            if (event == 3 || event == 4) continue;
            if ("item".equals(parser.getName())) {
                String name = parser.getAttributeValue(null, "name");
                boolean granted = parser.getAttributeBoolean(null, "granted", true);
                int flags = parser.getAttributeIntHex(null, "flags", 0);
                if (runtime) state.putPermissionState(new LegacyPermissionState.PermissionState(name, true, granted, flags), 10);
                else for (int user : new int[] {10, 0}) state.putPermissionState(new LegacyPermissionState.PermissionState(name, false, granted, flags), user);
            } else if (!runtime) {
                int skippedDepth = parser.getDepth();
                while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > skippedDepth)) {}
            }
        }
    }
    private static void rejects(int appId, int[] users, byte[] bytes) {
        try { PackageLegacyPermissions.restore(appId, users, bytes); throw new AssertionError("malformed permission capture accepted"); }
        catch (IllegalArgumentException expected) {}
    }
}
