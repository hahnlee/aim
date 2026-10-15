package dev.aim.server;

import com.android.server.LocalServices;
import com.android.server.pm.UserManagerInternal;
import java.util.Objects;

/** Original resolve flag policy; UM unlock state is owned by the live original user service. */
public final class NativePMResolveFlags {
    private final PackageSnapshots.ComputerSnapshot packages;
    private final boolean safeMode;
    private final UserManagerInternal users;
    public NativePMResolveFlags(PackageSnapshots.ComputerSnapshot packages, long version, boolean safeMode,
            UserManagerInternal users) {
        this.packages = Objects.requireNonNull(packages);
        if (version != packages.getVersion()) throw new IllegalArgumentException("safe-mode capture differs");
        this.safeMode = safeMode;
        this.users = Objects.requireNonNull(users);
    }
    public long update(long flags, int user, int callingUid, boolean wantInstant, boolean implicitCapture) {
        packages.getVersion();
        if (safeMode || implicitCapture) flags |= 0x100000L;
        if (packages.getInstantAppPackageName(callingUid) != null) flags |= 0x1000000L | 0x800000L;
        else {
            boolean requested = (flags & 0x800000L) != 0;
            boolean allowed = wantInstant || (requested && packages.canViewInstantApps(callingUid, user));
            flags &= ~(0x1000000L | 0x2000000L);
            if (!allowed) flags &= ~0x800000L;
        }
        if ((flags & (0x40000L | 0x80000L)) == 0) {
            if (LocalServices.getService(UserManagerInternal.class) != users)
                throw new IllegalStateException("original user unlock owner replaced");
            flags |= users.isUserUnlockingOrUnlocked(user) ? 0x40000L | 0x80000L : 0x80000L;
        }
        return flags;
    }
}
