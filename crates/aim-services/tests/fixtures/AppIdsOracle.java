package com.android.server.pm;

public final class AppIdsOracle {
    public static void main(String[] args) {
        AppIdSettingMap ids = new AppIdSettingMap();
        SettingBase a = new SharedUserSetting("a", 1, 8);
        SettingBase b = new SharedUserSetting("b", 1, 8);
        System.out.println(ids.registerExistingAppId(10002, a, "a"));
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        ids.removeSetting(10001);
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        ids.replaceSetting(10001, a);
        System.out.println(ids.getSetting(10001) == a);
        ids.replaceSetting(1000, a);
        System.out.println(ids.getSetting(1000) == a);
        ids.removeSetting(10010);
        System.out.println(ids.acquireAndRegisterNewAppId(b));

        AppIdSettingMap restart = new AppIdSettingMap();
        restart.registerExistingAppId(10002, a, "a");
        System.out.println(restart.acquireAndRegisterNewAppId(b));
        AppIdSettingMap full = new AppIdSettingMap();
        int last = -1;
        for (int i = 0; i < 10000; i++) last = full.acquireAndRegisterNewAppId(a);
        System.out.println(last);
        System.out.println(full.acquireAndRegisterNewAppId(b));
        full.removeSetting(19999);
        System.out.println(full.acquireAndRegisterNewAppId(b));
    }
}
