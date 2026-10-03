package com.android.server.pm;

import com.android.internal.pm.parsing.pkg.PackageImpl;
import com.android.server.pm.pkg.AndroidPackage;
import java.io.File;
import java.util.UUID;

/** Exercise original shared-UID seInfo SDK lifetime with original parsed objects. */
public final class SharedSeInfoOracle {
    private static PackageSetting member(String name, Integer sdk) {
        var setting = new PackageSetting(name, null, new File("/data/app/" + name),
                0, 0, new UUID(1, 1));
        if (sdk != null) {
            var pkg = (PackageImpl)PackageImpl.forTesting(name);
            pkg.setTargetSdkVersion(sdk);
            setting.setPkg((AndroidPackage)(Object)pkg);
        }
        return setting;
    }
    private static void check(SharedUserSetting group, int sdk) {
        if (group.getSeInfoTargetSdkVersion() != sdk) {
            throw new AssertionError("shared seInfo SDK: " + group.getSeInfoTargetSdkVersion() + " expected " + sdk);
        }
    }
    public static void main(String[] args) {
        var group = new SharedUserSetting("group", 0, 0);
        var absent = member("absent", null);
        var a = member("a", 36); var b = member("b", 28); var c = member("c", 19);
        check(group, 10000);
        group.addPackage(absent); group.addPackage(a); check(group, 10000);
        group.fixSeInfoLocked(); check(group, 36);
        if (!"default:targetSdkVersion=36".equals(((com.android.server.pm.pkg.PackageState)a).getSeInfo())) {
            throw new AssertionError("original boot seInfo label");
        }
        group.addPackage(b); check(group, 36);
        group.fixSeInfoLocked(); check(group, 28);
        if (!"default:targetSdkVersion=28".equals(((com.android.server.pm.pkg.PackageState)a).getSeInfo())
                || !((com.android.server.pm.pkg.PackageState)a).getSeInfo().equals(((com.android.server.pm.pkg.PackageState)b).getSeInfo())) {
            throw new AssertionError("original shared boot label agreement");
        }
        group.addPackage(c); check(group, 28);
        group.removePackage(b); check(group, 28);
        group.removePackage(a); group.removePackage(absent); group.removePackage(c); check(group, 28);
        var d = member("d", 35); var e = member("e", 24);
        group.addPackage(d); check(group, 35);
        group.addPackage(e); check(group, 35);
        group.fixSeInfoLocked(); check(group, 24);
        var reboot = new SharedUserSetting("group", 0, 0);
        reboot.addPackage(member("d", null)); reboot.addPackage(member("e", null));
        check(reboot, 10000);
        reboot.fixSeInfoLocked(); check(reboot, 10000);
        System.out.println("SHARED_SEINFO first parsed boot runtime removal empty reboot");
    }
}
