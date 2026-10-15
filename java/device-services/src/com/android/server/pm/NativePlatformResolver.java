package com.android.server.pm;

import android.content.Context;
import dev.aim.server.NativePackageBootstrap;
import dev.aim.server.PackageSnapshots;

/** Resolver state is computed from the native scan's actual committed order. */
public final class NativePlatformResolver {
    private NativePlatformResolver() { }
    public static NativePackageBootstrap.ResolverOwner create(Context context,PackageSnapshots.Store packages) {
        try(var snapshot=packages.computer()) {
            var activity=snapshot.getNativeResolverActivity();
            if(activity==null)throw new IllegalStateException("Native resolver activity owner unavailable");
            return new NativePackageBootstrap.ResolverOwner(activity,snapshot.isNativeResolverReplaced());
        }
    }
}
