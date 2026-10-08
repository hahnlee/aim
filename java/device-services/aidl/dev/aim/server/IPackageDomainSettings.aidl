package dev.aim.server;
import dev.aim.server.IPackageDomainSettingsChanged;
interface IPackageDomainSettings {
    void seed(in byte[] nativeSettings, IPackageDomainSettingsChanged changes);
    byte[] capture();
    void reconcilePackages();
    void close();
}
