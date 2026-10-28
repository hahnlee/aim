package dev.aim.server;

/** Signing reads must not reconstruct a setting or its parsed package code. */
public final class RetainedSigningOracle {
    public static void verify(com.android.server.pm.PackageSetting original) {
        int[] copies = {0};
        var replica = new PackageStateReplica(() -> {
            if (++copies[0] != 1) throw new AssertionError("signing query reconstructed package code");
            return original;
        }, java.util.Map.of(), 0);
        for (int i = 0; i < 2; i++) {
            if (!replica.getSigningDetails().equals(original.getSigningDetails())
                    || !replica.getSigningInfo().getSigningDetails().equals(original.getSigningDetails()))
                throw new AssertionError("retained signing differs");
        }
        if (copies[0] != 1) throw new AssertionError("retained signing reconstructed setting");
    }
}
