package dev.aim.server;

import android.content.pm.Signature;
import android.content.pm.SigningDetails;
import android.os.Parcel;
import java.util.Objects;

/** Versioned full signing owner: ordinary SigningDetails parcels omit capability flags. */
public final class PackageSigningDetails {
    private PackageSigningDetails() {}
    public static byte[] encode(SigningDetails details) {
        Objects.requireNonNull(details);
        Parcel out = Parcel.obtain();
        try {
            out.writeInt(1);
            out.writeInt(details == SigningDetails.UNKNOWN ? 1 : 0);
            if (details != SigningDetails.UNKNOWN) {
                write(out, Objects.requireNonNull(details.getSignatures()));
                out.writeInt(details.getSignatureSchemeVersion());
                var keys = details.getPublicKeys();
                out.writeInt(keys == null ? -1 : keys.size());
                if (keys != null) for (var key : keys) out.writeByteArray(key == null ? null : key.getEncoded());
                var past = details.getPastSigningCertificates();
                if (past == null) out.writeInt(-1); else write(out, past);
            }
            return out.marshall();
        } finally { out.recycle(); }
    }
    private static void write(Parcel out, Signature[] certificates) {
        out.writeInt(certificates.length);
        for (var certificate : certificates) {
            out.writeByteArray(Objects.requireNonNull(certificate).toByteArray());
            out.writeInt(certificate.getFlags());
        }
    }
}
