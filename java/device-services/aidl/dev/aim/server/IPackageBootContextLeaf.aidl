package dev.aim.server;
import dev.aim.server.IPackageFilteringUpdates;
/** Independent UM/resources/device-policy inputs for the first native Capture. */
interface IPackageBootContextLeaf {
    byte[] captureBootContext();
    boolean deviceProvisioned();
    boolean queryFilteringDisabled();
    boolean enterSystemReady();
    void registerFilteringUpdates(IPackageFilteringUpdates owner);
    void beginSystemReady();
    boolean userUnlockingOrUnlocked(int userId);
}
