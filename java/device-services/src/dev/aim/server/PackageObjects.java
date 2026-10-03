package dev.aim.server;

import android.content.pm.Signature;
import android.content.pm.SigningDetails;
import android.util.ArraySet;
import com.android.internal.pm.parsing.pkg.PackageImpl;
import com.android.server.pm.parsing.PackageCacher;
import java.util.Arrays;
import java.util.Objects;

/** Original package objects decoded from native collected code before publication. */
public final class PackageObjects {
    private PackageObjects() {}

    public static PackageImpl fromCache(byte[] cache, byte[][] pastCertificates, int[] capabilities) {
        PackageImpl pkg = (PackageImpl) PackageCacher.fromCacheEntryStatic(cache);
        pkg.setSigningDetails(restoreSigning(pkg.getSigningDetails(), pastCertificates, capabilities));
        return pkg;
    }

    // Signature's Parcel format omits flags. Bind the separate flags to certificate bytes
    // and copy every mutable array/signature, leaving the decoded object untouched.
    public static SigningDetails restoreSigning(SigningDetails cached,
            byte[][] pastCertificates, int[] capabilities) {
        Objects.requireNonNull(cached, "cached signing");
        Signature[] past = cached.getPastSigningCertificates();
        if ((past == null) != (pastCertificates == null)
                || (past == null) != (capabilities == null)
                || (past != null && (past.length != pastCertificates.length
                    || past.length != capabilities.length))) {
            throw new IllegalArgumentException("signing lineage metadata mismatch");
        }
        if (cached == SigningDetails.UNKNOWN) return cached;
        Signature[] restoredPast = copy(past);
        if (past != null) {
            for (int i = 0; i < past.length; i++) {
                if (pastCertificates[i] == null
                        || !Arrays.equals(past[i].toByteArray(), pastCertificates[i])) {
                    throw new IllegalArgumentException("signing lineage certificate mismatch");
                }
                restoredPast[i].setFlags(capabilities[i]);
            }
        }
        ArraySet<java.security.PublicKey> keys = null;
        if (cached.getPublicKeys() != null) {
            keys = new ArraySet<>();
            keys.addAll(cached.getPublicKeys());
        }
        return new SigningDetails(copy(cached.getSignatures()),
            cached.getSignatureSchemeVersion(), keys, restoredPast);
    }

    private static Signature[] copy(Signature[] values) {
        if (values == null) return null;
        Signature[] result = new Signature[values.length];
        for (int i = 0; i < values.length; i++) {
            result[i] = new Signature(values[i]);
        }
        return result;
    }
}
