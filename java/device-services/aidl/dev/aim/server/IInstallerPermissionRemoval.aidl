package dev.aim.server;
/** Captured native package state, consumed by the original permission owner. */
interface IInstallerPermissionRemoval {
    void apply(boolean codeRemoved, boolean clearGrants);
}
