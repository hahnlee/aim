// Compile-only original API; runtime uses the original independent resolver helper.
package com.android.server.pm;
public abstract class InstantAppResolver {
    private InstantAppResolver() { throw new RuntimeException("stub"); }
    public static void doInstantAppResolutionPhaseTwo(android.content.Context context, Computer computer, UserManagerService users, InstantAppResolverConnection connection, android.content.pm.InstantAppRequest request, android.content.pm.ActivityInfo installer, android.os.Handler handler) { throw new RuntimeException("stub"); }
}
