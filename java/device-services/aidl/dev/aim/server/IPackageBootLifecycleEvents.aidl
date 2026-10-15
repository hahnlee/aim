package dev.aim.server;
interface IPackageBootLifecycleEvents {
    void volumeReady(String volumeUuid);
    void overlayChanged(String packageName, int userId);
}
