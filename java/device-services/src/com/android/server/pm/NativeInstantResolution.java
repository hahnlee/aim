package com.android.server.pm;

import android.content.Context;
import android.content.Intent;
import android.content.pm.AuxiliaryResolveInfo;
import android.content.pm.InstantAppRequest;
import android.os.Bundle;
import android.os.Handler;
import java.util.Objects;
import java.util.function.Supplier;

/** Original phase-two resolver/AM leaf, fed by the native Computer capture. */
public final class NativeInstantResolution {
    private final Context context;
    private final Handler handler;
    private final UserManagerService users;
    private final InstantAppResolverConnection connection;
    private final Supplier<NativeComputer> computers;
    public NativeInstantResolution(Context context, Handler handler, UserManagerService users,
            android.content.ComponentName resolverComponent, Supplier<NativeComputer> computers) {
        this.context = Objects.requireNonNull(context);
        this.handler = Objects.requireNonNull(handler);
        this.users = Objects.requireNonNull(users);
        this.connection = resolverComponent == null ? null : new InstantAppResolverConnection(context,
                resolverComponent, "android.intent.action.RESOLVE_INSTANT_APP_PACKAGE");
        this.computers = Objects.requireNonNull(computers);
    }
    public void request(AuxiliaryResolveInfo response, Intent intent, String resolvedType,
            String caller, String feature, boolean requesterInstant, Bundle verification, int user) {
        InstantAppRequest request = new InstantAppRequest(response, intent, resolvedType,
                caller, feature, requesterInstant, user, verification, false,
                response.hostDigestPrefixSecure, response.token);
        if (!handler.post(() -> {
            NativeComputer computer = Objects.requireNonNull(computers.get());
            // Original phase-two callback retains this Computer until it has completed.
            // Its Cleaner releases the immutable native capture after the callback becomes unreachable.
            InstantAppResolver.doInstantAppResolutionPhaseTwo(context, computer, users,
                    connection, request, computer.getInstantAppInstallerInfo().activityInfo, handler);
        })) throw new IllegalStateException("instant resolver handler has stopped");
    }
}
