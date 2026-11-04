package dev.aim.server;

/** Synchronous SystemServiceManager lifecycle delivery, before USER_UNLOCKED dispatch. */
interface ISystemUserLifecycle {
    void onUserUnlocking(int userId);
    void onShutdown();
}
