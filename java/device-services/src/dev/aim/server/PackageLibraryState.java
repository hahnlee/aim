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
                result.add(declaration(SharedLibraryInfo.CREATOR.createFromParcel(in)));
            }
            if (in.dataAvail() != 0) throw new IllegalArgumentException("trailing library bytes");
            return result;
        } finally { in.recycle(); }
    }

    // readParcelableList normalizes null optional dependents to allocated-empty.
    // Native declaration owners use the original constructor's null optional/
    // certificate fields; reconstruct them instead of importing that normalization.
    private static SharedLibraryInfo declaration(SharedLibraryInfo decoded) {
        List<SharedLibraryInfo> dependencies = null;
        if (decoded.getDependencies() != null) {
            dependencies = new ArrayList<>();
            for (var dependency : decoded.getDependencies()) dependencies.add(declaration(dependency));
        }
        var dependents = decoded.getDependentPackages();
        return new SharedLibraryInfo(decoded.getPath(), decoded.getPackageName(),
            decoded.getPath() == null ? decoded.getAllCodePaths() : null,
            decoded.getName(), decoded.getLongVersion(), decoded.getType(), decoded.getDeclaringPackage(),
            dependents.isEmpty() ? null : dependents, dependencies, decoded.isNative());
    }
}
