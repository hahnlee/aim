package dev.aim.server;

import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;
import android.util.ArrayMap;
import com.android.permission.persistence.RuntimePermissionsState;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/** Original RuntimePermissionsState assembled from the native locked Settings/metadata owner. */
public final class NativePMLegacyRuntimePermissions implements NativePackageManagerInternal.LegacyRuntimeOwner {
    private final IPackageInternalHost host;
    public NativePMLegacyRuntimePermissions(IPackageInternalHost host) { this.host = Objects.requireNonNull(host); }
    public RuntimePermissionsState state(int user) {
        byte[] record;
        try { record = host.getLegacyRuntimePermissionsStateRecord(user, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        return decode(record);
    }
    static RuntimePermissionsState decode(byte[] record) {
        if (record == null) throw new IllegalStateException("legacy runtime permission owner unavailable");
        Parcel parcel = Parcel.obtain();
        try {
            parcel.unmarshall(record, 0, record.length); parcel.setDataPosition(0);
            if (parcel.readInt() != 1) throw new IllegalArgumentException("legacy runtime record version");
            int version = parcel.readInt(); String fingerprint = parcel.readString();
            var packages = owners(parcel); var shared = owners(parcel);
            if (parcel.dataAvail() != 0) throw new IllegalArgumentException("legacy runtime record trailing data");
            return new RuntimePermissionsState(version, fingerprint, packages, shared);
        } finally { parcel.recycle(); }
    }
    @Override public RuntimePermissionsState getLegacyPermissionsState(int user) { return state(user); }
    @Override public int getLegacyPermissionsVersion(int user) { return state(user).getVersion(); }
    public int version(int user) { return state(user).getVersion(); }
    private static Map<String,List<RuntimePermissionsState.PermissionState>> owners(Parcel parcel) {
        int count = count(parcel); var result = new ArrayMap<String,List<RuntimePermissionsState.PermissionState>>();
        for (int i = 0; i < count; i++) {
            String name = Objects.requireNonNull(parcel.readString());
            if (result.containsKey(name)) throw new IllegalArgumentException("duplicate legacy permission owner");
            int size = count(parcel); var permissions = new ArrayList<RuntimePermissionsState.PermissionState>(size);
            for (int j = 0; j < size; j++) permissions.add(new RuntimePermissionsState.PermissionState(
                    Objects.requireNonNull(parcel.readString()), parcel.readBoolean(), parcel.readInt()));
            result.put(name, permissions);
        }
        return result;
    }
    private static int count(Parcel parcel) {
        int count = parcel.readInt();
        if (count < 0 || count > parcel.dataAvail()/4) throw new IllegalArgumentException("legacy permission record count");
        return count;
    }
}
