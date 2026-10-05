package com.android.server.pm;

public final class NativeDisplacedReadOracle {
    public static void main(String[] args) {
        try {
            for (int index = 0; index < 6; index++) {
                var data = new java.io.File(args[0], "case-" + index + "/persisted");
                var settings = new Settings(data, null, null, null, null, new PackageManagerTracedLock());
                if (!settings.readLPw(null, java.util.List.of())) throw new AssertionError("native data became first boot");
                int keep = index % 3;
                var incoming = settings.getPackagesLocked().get("com.google.android.gsf");
                var original = settings.getPackagesLocked().get("fixture.original.gsf");
                var oldGroup = settings.getSettingLPr(10002);
                if ((incoming != null) != (keep != 0) || (oldGroup != null) != (keep != 0)
                        || original == null || original.getAppId() != 10003
                        || settings.getSettingLPr(10003) == null)
                    throw new AssertionError("native displaced reader identity differs: " + index);
                if (keep != 0 && (((SharedUserSetting) oldGroup).getPackageStates().size() != (keep == 1 ? 2 : 1)
                        || !((SharedUserSetting) oldGroup).getPackageStates().contains(incoming)))
                    throw new AssertionError("native saved incoming did not rejoin old group");
                if (settings.getDisabledSystemPackagesLocked().size() != (keep == 2 ? 1 : 0))
                    throw new AssertionError("native factory read inventory differs");
                if (index >= 3 && !(settings.getSettingLPr(10003) instanceof SharedUserSetting))
                    throw new AssertionError("native original shared slot differs");
            }
            System.out.println("native displaced read contracts: 6 cases");
            System.exit(0);
        } catch (Throwable error) { error.printStackTrace(System.out); System.exit(1); }
    }
}
