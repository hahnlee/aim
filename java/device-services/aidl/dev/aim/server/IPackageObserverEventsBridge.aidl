package dev.aim.server;
/** Native package publication feeds the original non-Binder LocalServices observers. */
interface IPackageObserverEventsBridge {
    void packageAdded(String packageName, int uid);
    void packageChanged(String packageName, int uid);
    void packageRemoved(String packageName, int uid);
    void revoke();
}
