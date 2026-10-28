package dev.aim.server;

/** Cold reconciliation over the native owner's access-state snapshot. */
interface IPermissionBootPolicy {
    int evaluatePermission(String name, int protectionLevel, int infoFlags,
            int targetSdkVersion, int oldFlags, int newFlags);
}
