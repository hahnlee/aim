package dev.aim.server;

import android.content.pm.Signature;
import android.content.pm.SigningDetails;
import android.os.Parcel;
import android.os.Parcelable;
import java.util.Objects;

/** Immutable saved package/shared-UID signing, separate from collected code. */
public final class PackageSigningState implements Parcelable {
    private final long version;
    private final String name;
    private final int appId;
    private final boolean disabled;
    private final String sharedGroup;
    private final Signing packageSigning;
    private final Signing sharedSigning;

    private PackageSigningState(Parcel in) {
        version = in.readLong();
        name = Objects.requireNonNull(in.readString());
        appId = in.readInt();
        disabled = in.readBoolean();
        sharedGroup = in.readString();
        packageSigning = Signing.read(in);
        sharedSigning = Signing.read(in);
        if (sharedGroup == null && sharedSigning != null) {
            throw new IllegalArgumentException("signing group metadata mismatch");
        }
    }

    public long getVersion() { return version; }
    public String getPackageName() { return name; }
    public int getAppId() { return appId; }
    public boolean isDisabled() { return disabled; }
    public String getSharedGroupName() { return sharedGroup; }
    public SigningDetails getPackageSigningDetails() { return Signing.details(packageSigning); }
    public SigningDetails getSharedSigningDetails() {
        return sharedGroup == null ? null : Signing.details(sharedSigning);
    }

    private static final class Signing {
        private final int scheme;
        private final byte[][] current;
        private final byte[][] past;
        private final int[] capabilities;

        private Signing(Parcel in) {
            scheme = in.readInt();
            int count = count(in, false);
            current = new byte[count][];
            for (int i = 0; i < count; i++) current[i] = Objects.requireNonNull(in.createByteArray());
            count = count(in, true);
            past = count == -1 ? null : new byte[count][];
            capabilities = count == -1 ? null : new int[count];
            for (int i = 0; i < count; i++) {
                past[i] = Objects.requireNonNull(in.createByteArray());
                capabilities[i] = in.readInt();
            }
        }

        private static int count(Parcel in, boolean nullable) {
            int count = in.readInt();
            if (count < (nullable ? -1 : 0) || count > in.dataAvail() / (nullable ? 8 : 4)) {
                throw new IllegalArgumentException("invalid signing certificate count");
            }
            return count;
        }

        static Signing read(Parcel in) { return in.readBoolean() ? new Signing(in) : null; }

        private static Signature[] signatures(byte[][] certificates, int[] flags) {
            if (certificates == null) return null;
            Signature[] result = new Signature[certificates.length];
            for (int i = 0; i < result.length; i++) {
                result[i] = new Signature(certificates[i].clone());
                if (flags != null) result[i].setFlags(flags[i]);
            }
            return result;
        }

        static SigningDetails details(Signing signing) {
            if (signing == null) return SigningDetails.UNKNOWN;
            try {
                // The original constructor derives the complete public-key set.
                return new SigningDetails(signatures(signing.current, null), signing.scheme,
                        signatures(signing.past, signing.capabilities));
            } catch (java.security.cert.CertificateException failure) {
                throw new IllegalArgumentException("invalid saved signing certificates", failure);
            }
        }

        static void write(Parcel out, Signing signing) {
            out.writeBoolean(signing != null);
            if (signing == null) return;
            out.writeInt(signing.scheme);
            out.writeInt(signing.current.length);
            for (byte[] certificate : signing.current) out.writeByteArray(certificate);
            out.writeInt(signing.past == null ? -1 : signing.past.length);
            if (signing.past != null) {
                for (int i = 0; i < signing.past.length; i++) {
                    out.writeByteArray(signing.past[i]);
                    out.writeInt(signing.capabilities[i]);
                }
            }
        }
    }

    @Override
    public void writeToParcel(Parcel out, int flags) {
        out.writeLong(version);
        out.writeString(name);
        out.writeInt(appId);
        out.writeBoolean(disabled);
        out.writeString(sharedGroup);
        Signing.write(out, packageSigning);
        Signing.write(out, sharedSigning);
    }

    @Override
    public int describeContents() { return 0; }
    public static final Parcelable.Creator<PackageSigningState> CREATOR = new Parcelable.Creator<>() {
        @Override
        public PackageSigningState createFromParcel(Parcel in) { return new PackageSigningState(in); }
        @Override
        public PackageSigningState[] newArray(int size) { return new PackageSigningState[size]; }
    };
}
