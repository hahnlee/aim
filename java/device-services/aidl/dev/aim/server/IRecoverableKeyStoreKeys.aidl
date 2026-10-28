package dev.aim.server;

/** Cold recoverable-key calls; caller UID is authenticated by the native lock_settings owner. */
interface IRecoverableKeyStoreKeys {
    @nullable String generateKey(int uid, @nullable String alias);
    void removeKey(int uid, @nullable String alias);
}
