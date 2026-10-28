package dev.aim.server;

/** Owner facts for the custom permissions installed by ActiveServices. */
interface IForegroundServiceTypeHost {
    const int SYSTEM_ALLOWLIST = 0;
    const int DEMO = 1;
    const int DISALLOW_APPS_CONTROL = 2;
    const int DEVICE_OWNER = 3;
    const int PROFILE_OWNER = 4;
    const int PERSISTENT = 5;
    const int SYSTEM_MODULE = 6;
    const int CARRIER = 7;
    const int PROTECTED = 8;
    const int ACTIVE_ADMIN = 9;
    const int VPN = 10;
    const int PLATFORM_VPN = 11;
    const int DIALER = 12;
    const int EMERGENCY = 13;
    const int USER_ALLOWLIST = 14;
    const int COMPANION = 15;
    const int INSTALLER = 16;
    const int MEDIA_PROJECTION = 17;
    boolean hasForegroundServiceExemption(int fact, int uid, String packageName);
}
