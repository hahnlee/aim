// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;

final class AppIdSettingMap {
    AppIdSettingMap() {}
    public boolean registerExistingAppId(int appId, SettingBase setting, Object name) {
        throw new UnsupportedOperationException();
    }
    public SettingBase getSetting(int appId) { throw new UnsupportedOperationException(); }
    public void removeSetting(int appId) { throw new UnsupportedOperationException(); }
    public void replaceSetting(int appId, SettingBase setting) {
        throw new UnsupportedOperationException();
    }
    public int acquireAndRegisterNewAppId(SettingBase setting) {
        throw new UnsupportedOperationException();
    }
}
