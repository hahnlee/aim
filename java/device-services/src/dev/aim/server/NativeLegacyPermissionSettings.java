package dev.aim.server;

import android.content.pm.PermissionInfo;
import android.os.Parcel;
import com.android.server.pm.permission.LegacyPermission;
import com.android.server.pm.permission.LegacyPermissionSettings;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/** Bootstrap's stable original legacy definitions owner, initialized from native Settings. */
public final class NativeLegacyPermissionSettings {
    private NativeLegacyPermissionSettings() {}
    public static LegacyPermissionSettings create(PackageSnapshots.ComputerSnapshot packages) {
        long version = packages.getVersion();
        byte[] record = Objects.requireNonNull(packages.getLegacyPermissionDefinitionsRecord(),
                "native legacy permission definitions unavailable");
        return create(version,record);
    }
    public static LegacyPermissionSettings create(long version,byte[] record) {
        Objects.requireNonNull(record,"native legacy permission definitions unavailable");
        Parcel parcel = Parcel.obtain();
        try {
            parcel.unmarshall(record, 0, record.length); parcel.setDataPosition(0);
            if (parcel.readInt() != 1 || parcel.readLong() != version)
                throw new IllegalArgumentException("legacy permission definitions capture differs");
            var permissions = read(parcel); var trees = read(parcel);
            if (parcel.dataAvail() != 0) throw new IllegalArgumentException("legacy permission definitions trailing data");
            var owner = new LegacyPermissionSettings();
            owner.replacePermissions(permissions); owner.replacePermissionTrees(trees);
            return owner;
        } finally { parcel.recycle(); }
    }
    private static List<LegacyPermission> read(Parcel parcel) {
        int count = parcel.readInt();
        if (count < 0 || count > parcel.dataAvail()/4) throw new IllegalArgumentException("legacy permission definition count");
        var result = new ArrayList<LegacyPermission>(count);
        var names = new java.util.HashSet<String>();
        for (int i = 0; i < count; i++) {
            var info = new PermissionInfo(); info.name = Objects.requireNonNull(parcel.readString());
            if (!names.add(info.name)) throw new IllegalArgumentException("duplicate legacy permission definition");
            info.packageName = Objects.requireNonNull(parcel.readString()); info.protectionLevel = parcel.readInt();
            int type = parcel.readInt(); int uid = parcel.readInt(); int[] gids = Objects.requireNonNull(parcel.createIntArray());
            info.icon = parcel.readInt(); info.nonLocalizedLabel = parcel.readString();
            if (type < 0 || type > 2) throw new IllegalArgumentException("legacy permission owner type");
            result.add(new LegacyPermission(info, type, uid, gids));
        }
        return result;
    }
}
