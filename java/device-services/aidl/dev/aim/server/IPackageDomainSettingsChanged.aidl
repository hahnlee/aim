package dev.aim.server;
/** Original domain owner publishes its serialized settings to native persistence. */
interface IPackageDomainSettingsChanged { void changed(in byte[] settings); }
