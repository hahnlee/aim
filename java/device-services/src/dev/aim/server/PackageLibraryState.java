package dev.aim.server;

import android.content.pm.SharedLibraryInfo;
import android.os.Parcel;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/** Immutable transport inputs; every getter decodes detached original objects. */
public final class PackageLibraryState {
    private final long version;
    private final String name;
    private final int appId;
    private final String[] files;
    private final byte[] libraries;

    private PackageLibraryState(Parcel in) {
        version = in.readLong(); name = Objects.requireNonNull(in.readString()); appId = in.readInt();
        files = Objects.requireNonNull(in.createStringArray());
        for (String path : files) Objects.requireNonNull(path);
        libraries = Objects.requireNonNull(in.createByteArray());
        var decoded = getLibraries();
        Parcel out = Parcel.obtain();
        try {
            out.writeInt(decoded.size());
            for (var library : decoded) { out.writeInt(1); library.writeToParcel(out, 0); }
            if (!java.util.Arrays.equals(libraries, out.marshall())) throw new IllegalArgumentException("library owner cannot be reproduced");
        } finally { out.recycle(); }
    }
    public static PackageLibraryState read(Parcel in) { return new PackageLibraryState(in); }
    public long getVersion() { return version; }
    public String getPackageName() { return name; }
    public int getAppId() { return appId; }
    public List<String> getFiles() { return new ArrayList<>(java.util.Arrays.asList(files)); }
    public List<SharedLibraryInfo> getLibraries() {
        Parcel in = Parcel.obtain();
        try {
            in.unmarshall(libraries, 0, libraries.length); in.setDataPosition(0);
            int count = in.readInt();
            if (count < 0 || count > in.dataAvail() / 4) throw new IllegalArgumentException("invalid dependency count");
            var result = new ArrayList<SharedLibraryInfo>(count);
            for (int i = 0; i < count; i++) {
                if (in.readInt() != 1) throw new IllegalArgumentException("missing library object");
                result.add(original(in));
            }
            if (in.dataAvail() != 0) throw new IllegalArgumentException("trailing library bytes");
            return result;
        } finally { in.recycle(); }
    }

    // The original Parcel constructor normalizes null optional dependents. Preserve
    // captured constructor owners, then prove reproduction of the complete record.
    private SharedLibraryInfo original(Parcel in) {
        int start = in.dataPosition();
        var decoded = SharedLibraryInfo.CREATOR.createFromParcel(in);
        int end = in.dataPosition();
        byte[] bytes = java.util.Arrays.copyOfRange(libraries, start, end);
        if (matches(decoded, bytes)) return decoded;
        in.setDataPosition(start);
        String path = in.readString8();
        String packageName = in.readString8();
        int codeMarker = in.readInt();
        if (codeMarker != 0 && codeMarker != 1) throw new IllegalArgumentException("invalid code-path marker");
        List<String> codePaths = codeMarker == 0 ? null : java.util.Arrays.asList(in.createString8Array());
        String libraryName = in.readString8();
        long libraryVersion = in.readLong();
        int type = in.readInt();
        var declaring = in.readParcelable(null, android.content.pm.VersionedPackage.class);
        var dependents = in.readArrayList(null, android.content.pm.VersionedPackage.class);
        int count = in.readInt();
        if (count < -1 || count > in.dataAvail() / 4) throw new IllegalArgumentException("invalid nested dependency count");
        List<SharedLibraryInfo> dependencies = count == -1 ? null : new ArrayList<>(count);
        for (int i = 0; i < count; i++) {
            int marker = in.readInt();
            if (marker == 0) dependencies.add(null);
            else if (marker == 1) dependencies.add(original(in));
            else throw new IllegalArgumentException("invalid nested library marker");
        }
        boolean nativeLibrary = in.readBoolean();
        in.readParcelableList(new ArrayList<android.content.pm.VersionedPackage>(),
            android.content.pm.VersionedPackage.class.getClassLoader(), android.content.pm.VersionedPackage.class);
        var certificates = in.createStringArrayList();
        if (in.dataPosition() != end) throw new IllegalArgumentException("library record mismatch");
        // Restore nested constructor owners without changing the original decoded
        // optional/certificate fields. Its dependency list is a fresh original owner.
        if (dependencies != null && decoded.getDependencies() != null) {
            decoded.getDependencies().clear(); decoded.getDependencies().addAll(dependencies);
        }
        if (matches(decoded, bytes)) return decoded;
        var restored = certificates == null
            ? new SharedLibraryInfo(path, packageName, codePaths, libraryName, libraryVersion,
                type, declaring, dependents, dependencies, nativeLibrary)
            : new SharedLibraryInfo(libraryName, libraryVersion, type, certificates);
        if (!matches(restored, bytes)) throw new IllegalArgumentException("library owner cannot be reproduced");
        return restored;
    }
    private static boolean matches(SharedLibraryInfo library, byte[] bytes) {
        Parcel out = Parcel.obtain();
        try { library.writeToParcel(out, 0); return java.util.Arrays.equals(bytes, out.marshall()); }
        finally { out.recycle(); }
    }
}
