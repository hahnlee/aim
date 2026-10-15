package dev.aim.server;
interface IApplicationDataBridge {
    void enforcePermission(String permission, int pid, int uid);
    void killAndWait(String packageName, int appId);
    boolean clearData(String packageName, int user);
    void checkMemory();
    void sendDeviceCustomizationReady();
    void deletePreloads();
}
