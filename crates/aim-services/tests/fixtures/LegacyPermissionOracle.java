import com.android.server.pm.permission.LegacyPermissionState;
import dev.aim.server.PackageLegacyPermissions;

/** Actual original LegacyPermissionState owners and production transport codec. */
public final class LegacyPermissionOracle {
    public static void verify(java.io.File directory) throws Exception {
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
    private static void rejects(int appId, int[] users, byte[] bytes) {
        try { PackageLegacyPermissions.restore(appId, users, bytes); throw new AssertionError("malformed permission capture accepted"); }
        catch (IllegalArgumentException expected) {}
    }
}
