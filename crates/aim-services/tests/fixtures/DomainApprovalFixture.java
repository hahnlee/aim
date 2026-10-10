package com.android.server.pm;
public final class DomainApprovalFixture {
    public static int approval(com.android.server.pm.verify.domain.DomainVerificationService service,
            PackageSetting setting, int mode, String host) {
        setting.getUserStates().remove(0);
        if (mode != -1) {
            var user = setting.modifyUserState(0);
            user.setInstalled(mode != 6); user.setEnabledState(mode >= 0 && mode <= 5 ? mode : 1);
            user.setInstantApp(mode == 7);
            user.setSuspendParams(null);
            if (mode == 8) {
                var suspensions = new android.util.ArrayMap<android.content.pm.UserPackage, com.android.server.pm.pkg.SuspendParams>();
                suspensions.put(android.content.pm.UserPackage.of(0, "suspender"), null);
                user.setSuspendParams(suspensions);
            }
        }
        var intent = new android.content.Intent("android.intent.action.VIEW");
        intent.setData(android.net.Uri.parse("https://" + host));
        intent.addCategory("android.intent.category.BROWSABLE");
        return service.approvalLevelForDomain((com.android.server.pm.pkg.PackageStateInternal) setting, intent, 0x10000L, 0);
    }
    public static void ownersUser(PackageSetting setting, int level, long time) {
        var user = setting.modifyUserState(0);
        user.setInstalled(level != 0); user.setEnabledState(1);
        user.setInstantApp(level == 5); user.setFirstInstallTimeMillis(time);
        setting.getUserStates().remove(10);
    }
    private DomainApprovalFixture() {}
}
