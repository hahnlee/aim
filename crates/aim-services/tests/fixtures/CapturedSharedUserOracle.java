package com.android.server.pm;

import dev.aim.server.PackageScanLease;
import com.android.server.pm.pkg.SharedUserApi;

/** Compare captured records and original frozen shared UID getter semantics. */
public final class CapturedSharedUserOracle {
    public static java.util.List<dev.aim.server.SharedUserReplica> verify(PackageScanLease lease, String packageName) throws Exception {
        var retained = new java.util.ArrayList<dev.aim.server.SharedUserReplica>();
        var names = lease.getSharedUserNames();
        if (names.isEmpty() || names != lease.getSharedUserNames()) throw new AssertionError("shared UID inventory cache");
        if (lease.getSharedUserData("unknown.shared") != null || lease.getSharedUserReplica("unknown.shared", true) != null) throw new AssertionError("unknown shared UID");
        for (String name : names) {
            var data = lease.getSharedUserData(name);
            if (data == null || data != lease.getSharedUserData(name)) throw new AssertionError("missing shared UID record");
            if (!data.getPackageNames().isEmpty() && !data.getPackageNames().contains(packageName)) {
                try { lease.getSharedUserReplica(name, true); throw new AssertionError("incomplete shared UID accepted"); }
                catch (java.io.IOException expected) {}
                continue;
            }
            var group = new SharedUserSetting(name, 0, data.isPrivileged() ? 8 : 0);
            group.mAppId = data.getAppId();
            group.signatures.mSigningDetails = data.getSigningDetails();
            for (String member : data.getPackageNames()) group.addPackage(lease.newScannedSetting(member, true));
            SharedUserApi original = (SharedUserApi)(Object)group.snapshot();
            var replica = lease.getSharedUserReplica(name, true);
            if (replica != lease.getSharedUserReplica(name, true)
                    || !replica.getName().equals(original.getName())
                    || replica.getAppId() != original.getAppId()
                    || replica.isPrivileged() != original.isPrivileged()
                    || replica.getSeInfoTargetSdkVersion() != original.getSeInfoTargetSdkVersion()
                    || !replica.getSigningDetails().equals(original.getSigningDetails())
                    || replica.getPackageStates().size() != original.getPackageStates().size()) throw new AssertionError("original frozen shared UID differs");
            retained.add(replica);
            var members = replica.getPackageStates();
            for (var member : members) {
                if (member != lease.getPackageStateReplica(member.getPackageName(), true)) throw new AssertionError("shared UID package identity");
            }
            members.clear();
            if (replica.getPackageStates().size() != data.getPackageNames().size()) throw new AssertionError("shared UID container escaped");
            var parcel = android.os.Parcel.obtain();
            try {
                data.writeToParcel(parcel, 0); byte[] before = parcel.marshall(); parcel.setDataPosition(0);
                var copy = dev.aim.server.SharedUserData.CREATOR.createFromParcel(parcel);
                if (parcel.dataAvail() != 0) throw new AssertionError("shared UID parcel tail");
                var out = android.os.Parcel.obtain();
                try {
                    copy.writeToParcel(out, 0);
                    if (!java.util.Arrays.equals(before, out.marshall())) throw new AssertionError("shared UID record roundtrip");
                } finally { out.recycle(); }
            } finally { parcel.recycle(); }
        }
        return java.util.List.copyOf(retained);
    }
}
