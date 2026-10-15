package dev.aim.server;
/** Original permission migration owner; native Settings owns its persisted input. */
interface IPermissionPersistenceBridge {
    byte[] captureDefinitions();
    void importNativeMigration();
}
