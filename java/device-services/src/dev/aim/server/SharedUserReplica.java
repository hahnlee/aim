package dev.aim.server;

import android.content.pm.SigningDetails;
import android.util.ArraySet;
import com.android.server.pm.pkg.SharedUserApi;
import java.util.List;

/** Original SharedUserApi backed by immutable captured UID and package owners. */
public final class SharedUserReplica implements SharedUserApi {
    private final SharedUserData state;
    private final List<PackageStateReplica> members;

    SharedUserReplica(SharedUserData state, List<PackageStateReplica> members) {
        this.state = state;
        this.members = List.copyOf(members);
        if (members.size() != state.getPackageNames().size()) throw new IllegalArgumentException("shared UID member count differs");
        for (int i = 0; i < members.size(); i++) {
            var member = members.get(i);
            if (!member.getPackageName().equals(state.getPackageNames().get(i))
                    || !member.hasSharedUser() || member.getSharedUserAppId() != state.getAppId()) {
                throw new IllegalArgumentException("shared UID member identity differs");
            }
        }
    }
    SharedUserReplica withPackages(java.util.Map<String, com.android.server.pm.pkg.PackageState> packages) {
        var updated = new java.util.ArrayList<PackageStateReplica>();
        for (int i = 0; i < members.size(); i++) {
            var member = members.get(i);
            updated.add(state.getMembers().get(i).retained() == null
                    ? (PackageStateReplica)java.util.Objects.requireNonNull(packages.get(member.getPackageName())) : member);
        }
        return new SharedUserReplica(state, updated);
    }

    List<PackageStateReplica> getCurrentPackageStates() {
        var current = new java.util.ArrayList<PackageStateReplica>();
        for (int i = 0; i < members.size(); i++) {
            if (state.getMembers().get(i).retained() == null) current.add(members.get(i));
        }
        return List.copyOf(current);
    }
    @Override public String getName() { return state.getName(); }
    @Override public int getAppId() { return state.getAppId(); }
    @Override public boolean isPrivileged() { return state.isPrivileged(); }
    @Override public int getSeInfoTargetSdkVersion() { return state.getSeInfoTargetSdkVersion(); }
    @Override public SigningDetails getSigningDetails() { return state.getSigningDetails(); }
    @Override public ArraySet<PackageStateReplica> getPackageStates() {
        var result = new ArraySet<PackageStateReplica>();
        for (var member : members) result.add(member);
        return result;
    }
}
