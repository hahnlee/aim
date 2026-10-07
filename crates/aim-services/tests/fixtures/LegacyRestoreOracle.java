/* Ported from android-16.0.0_r1 Settings reader loops.
 * Copyright (C) The Android Open Source Project, Apache License 2.0.
 * Install permission reads invoke original Settings; remaining restore loops are ported. */
package com.android.server.pm;

import com.android.modules.utils.TypedXmlPullParser;
import com.android.server.pm.permission.LegacyPermissionState;
import dev.aim.server.PackageLegacyPermissions;
import java.io.File;
import java.nio.file.Files;
import java.util.LinkedHashMap;
import java.util.Map;

public final class LegacyRestoreOracle {
    private final Map<String, PackageSetting> active = new LinkedHashMap<>();
    private final Map<String, PackageSetting> factory = new LinkedHashMap<>();
    private final Map<String, SharedUserSetting> shared = new LinkedHashMap<>();
    private final Map<Integer, SettingBase> visible = new LinkedHashMap<>();
    private final int[] users = {10, 0};

    private LegacyRestoreOracle() {
        seed("android.uid.system", 1000); seed("android.uid.phone", 1001);
        seed("android.uid.log", 1007); seed("android.uid.nfc", 1027);
        seed("android.uid.bluetooth", 1002); seed("android.uid.shell", 2000);
        seed("android.uid.se", 1031); seed("android.uid.networkstack", 1073);
        seed("android.uid.uwb", 1083); seed("android.uid.vendor.fixture", 2901);
    }
    private void seed(String name, int id) {
        var owner = new SharedUserSetting(name, 0, 0);
        owner.mAppId = id; shared.put(name, owner); visible.put(id, owner);
    }
    public static void readInstall(TypedXmlPullParser parser, LegacyPermissionState state) throws Exception {
        var users = new java.util.ArrayList<android.content.pm.UserInfo>();
        for (int id : new int[] {10, 0}) {
            var user = new android.content.pm.UserInfo(); user.id = id; users.add(user);
        }
        new Settings(java.util.Map.of()).readInstallPermissionsLPr(parser, state, users);
    }
    public static void verify(File directory, java.util.function.BiConsumer<com.android.server.pm.verify.domain.DomainVerificationService, PackageSetting> connect) throws Exception {
        verifyInstallBindings(directory);
        verifyFactoryEvents(directory);
        verifyNullableFactoryNames(directory);
        verifySharedSeedEvents(directory);
        verifyInitialRestrictions(directory,connect);
        for (int i = 0; i < 4; i++) {
            var root = new File(directory, "legacy-restore-" + i);
            var oracle = new LegacyRestoreOracle();
            try (var input = new java.io.FileInputStream(new File(root, "system/packages.xml"))) {
                var parser = android.util.Xml.resolvePullParser(input);
                while (parser.next() != 2) {}
                oracle.readSettings(parser);
            }
            for (var entry : oracle.active.entrySet()) oracle.compare(root, "active-" + entry.getKey(), entry.getValue());
            for (var entry : oracle.factory.entrySet()) oracle.compare(root, "factory-" + entry.getKey(), entry.getValue());
            for (var entry : oracle.shared.entrySet()) {
                var owner = entry.getValue();
                oracle.compareBytes(root, "shared-" + entry.getKey(), owner.mAppId, owner.getLegacyPermissionState());
            }
            long actual = java.util.Arrays.stream(root.listFiles()).filter(f -> f.getName().endsWith(".input")).count();
            if (actual != oracle.active.size() + oracle.factory.size() + oracle.shared.size()) throw new AssertionError("restored owner inventory differs");
        }
    }
    private void compare(File root, String stem, PackageSetting owner) throws Exception {
        compareBytes(root, stem, owner.getAppId(), owner.getLegacyPermissionState());
        boolean expected = Boolean.parseBoolean(new String(Files.readAllBytes(new File(root, stem + ".fixed").toPath()), java.nio.charset.StandardCharsets.UTF_8));
        if (expected != owner.isInstallPermissionsFixed()) throw new AssertionError("install permissions fixed differs: " + stem);
        if (stem.startsWith("factory-")) {
            byte[] bytes = Files.readAllBytes(new File(root, stem + ".setting").toPath());
            var in = android.os.Parcel.obtain();
            try {
                in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
                var data = dev.aim.server.PackageSettingData.read(in);
                var replica = CapturedPackageSetting.from(data, 1, true);
                if (in.dataAvail() != 0 || !replica.getDomainSetId().equals(com.android.server.pm.verify.domain.DomainVerificationManagerInternal.DISABLED_ID)
                        || replica.getAppId() != owner.getAppId() || !replica.hasSharedUser() || replica.isInstallPermissionsFixed()) throw new AssertionError("factory original metadata owner differs");
                compareBytes(root, stem, replica.getAppId(), replica.getLegacyPermissionState());
            } finally { in.recycle(); }
        }
    }
    private static void verifyInstallBindings(File directory) throws Exception {
        for (int index=0; ; index++) {
            var input=new File(directory,"install-binding-"+index+".xml"); if(!input.exists()) break;
            var data=new File(directory,"install-binding-original-"+index); var system=new File(data,"system"); system.mkdirs();
            Files.write(new File(system,"packages.xml").toPath(),Files.readAllBytes(input.toPath()));
            var reserve=new File(directory,"install-binding-"+index+".reserve");
            Files.write(new File(system,"packages.xml.reservecopy").toPath(),reserve.exists()?Files.readAllBytes(reserve.toPath()):"<packages/>".getBytes(java.nio.charset.StandardCharsets.UTF_8));
            var users=new java.util.ArrayList<android.content.pm.UserInfo>();
            for(int id:new int[]{10,0}) { var user=new android.content.pm.UserInfo(); user.id=id; users.add(user); }
            var settings=new Settings(data,null,null,null,null,new PackageManagerTracedLock());
            settings.readSettingsLPw(null,users,new android.util.ArrayMap<>());
            for(int id:new int[]{10001,10002}) {
                var owner=settings.getSettingLPr(id); var expected=new File(directory,"install-binding-"+index+"-"+id+".input");
                if(owner==null) { if(expected.exists()) throw new AssertionError("native UID owner unexpectedly exists"); }
                else {
                    byte[] actual=PackageLegacyPermissions.capture(id,new int[]{10,0},owner.getLegacyPermissionState());
                    if(!expected.exists() || !java.util.Arrays.equals(actual,Files.readAllBytes(expected.toPath()))) throw new AssertionError("original install UID binding differs "+index+":"+id);
                }
            }
            var pkg=settings.getPackagesLocked().get("p");
            String fixed=pkg==null?"absent":Boolean.toString(pkg.isInstallPermissionsFixed());
            if(!fixed.equals(new String(Files.readAllBytes(new File(directory,"install-binding-"+index+".fixed").toPath()),java.nio.charset.StandardCharsets.UTF_8))) throw new AssertionError("original install fixed flag differs "+index);
            settings.readLPw(null,java.util.List.of());
            for(String name:new String[]{"p","q"}) {
                var bound=settings.getPackagesLocked().get(name);
                String marker=bound==null?"absent":Boolean.toString(bound.isInstallPermissionsFixed());
                String expected=new String(Files.readAllBytes(new File(directory,"install-binding-"+index+".bound-fixed-"+name).toPath()),java.nio.charset.StandardCharsets.UTF_8);
                if(!marker.equals(expected)) throw new AssertionError("original pending fixed identity differs "+index+":"+name);
            }
            var retained=settings.getPackagesLocked().get("q");
            if(retained!=null) {
                for(int mode=0;mode<4;mode++) {
                    Files.write(new File(system,"packages.xml").toPath(),Files.readAllBytes(new File(directory,"install-binding-"+index+".reread-"+mode).toPath()));
                    Files.write(new File(system,"packages.xml.reservecopy").toPath(),"<packages/>".getBytes(java.nio.charset.StandardCharsets.UTF_8));
                    settings.readSettingsLPw(null,users,new android.util.ArrayMap<>());
                    var pkgAfter=settings.getPackagesLocked().get("q");
                    if(pkgAfter!=retained) throw new AssertionError("original retained package was replaced");
                    String actual=pkgAfter.hasSharedUser()+"|"+pkgAfter.isInstallPermissionsFixed();
                    String expected=new String(Files.readAllBytes(new File(directory,"install-binding-"+index+".reread-"+mode+".shared").toPath()),java.nio.charset.StandardCharsets.UTF_8);
                    if(!actual.equals(expected)) throw new AssertionError("retained shared relationship differs "+index+":"+mode);
                    byte[] permissions=PackageLegacyPermissions.capture(10002,new int[]{10,0},settings.getSettingLPr(10002).getLegacyPermissionState());
                    if(!java.util.Arrays.equals(permissions,Files.readAllBytes(new File(directory,"install-binding-"+index+".reread-"+mode+".permissions").toPath()))) throw new AssertionError("retained UID permission destination differs "+index+":"+mode);
                }
            }


        }
    }
    private static void verifyFactoryEvents(File directory) throws Exception {
        for(int index=0;;index++) {
            var input=new File(directory,"factory-event-"+index+".xml"); if(!input.exists()) break;
            var data=new File(directory,"factory-event-original-"+index);var system=new File(data,"system");system.mkdirs();
            Files.write(new File(system,"packages.xml").toPath(),Files.readAllBytes(input.toPath()));
            Files.write(new File(system,"packages.xml.reservecopy").toPath(),"<packages/>".getBytes(java.nio.charset.StandardCharsets.UTF_8));
            var users=new java.util.ArrayList<android.content.pm.UserInfo>(); for(int id:new int[]{10,0}){var user=new android.content.pm.UserInfo();user.id=id;users.add(user);}
            var settings=new Settings(data,null,null,null,null,new PackageManagerTracedLock());settings.readSettingsLPw(null,users,new android.util.ArrayMap<>());
            for(String name:new String[]{"p","f"}) {
                var p=settings.getDisabledSystemPkgLPr(name);
                String trace=p==null?"absent":p.getFlags()+"|"+p.getPrivateFlags()+"|"+p.getAppId()+"|"+p.hasSharedUser()+"|"+p.getDomainSetId()+"|"+p.getVersionCode()+"|"+p.getPathString()+"|"+p.isInstallPermissionsFixed();
                String expected=new String(Files.readAllBytes(new File(directory,"factory-event-"+index+"-"+name+".metadata").toPath()),java.nio.charset.StandardCharsets.UTF_8);
                if(!trace.equals(expected))throw new AssertionError("original factory metadata differs "+index+":"+name+" "+trace+" != "+expected);
                if(p!=null && !java.util.Arrays.equals(PackageLegacyPermissions.capture(p.getAppId(),new int[]{10,0},p.getLegacyPermissionState()),Files.readAllBytes(new File(directory,"factory-event-"+index+"-"+name+".permissions").toPath())))throw new AssertionError("original factory own permissions differ "+index+":"+name);
            }
            for(int id:new int[]{10001,10002,10003}) {
                var p=settings.getSettingLPr(id);var expected=new File(directory,"factory-event-"+index+"-"+id+".uid");
                if(p==null){if(expected.exists())throw new AssertionError("factory incorrectly registered UID");}
                else if(!expected.exists() || !java.util.Arrays.equals(PackageLegacyPermissions.capture(id,new int[]{10,0},p.getLegacyPermissionState()),Files.readAllBytes(expected.toPath())))throw new AssertionError("factory UID permission target differs "+index+":"+id);
            }
        }
    }
    private static void verifyNullableFactoryNames(File directory) throws Exception {
        String xml="<packages><updated-package codePath='/system/app/first' userId='10003' version='1'/><updated-package name='' codePath='/system/app/empty' userId='10004' version='2'/><updated-package codePath='/system/app/last' userId='10005' version='3'/></packages>";
        var data=new File(directory,"nullable-factory-original");var system=new File(data,"system");system.mkdirs();
        Files.write(new File(system,"packages.xml").toPath(),xml.getBytes(java.nio.charset.StandardCharsets.UTF_8));
        var settings=new Settings(data,null,null,null,null,new PackageManagerTracedLock());
        settings.readSettingsLPw(null,java.util.List.of(),new android.util.ArrayMap<>());
        var unnamed=settings.getDisabledSystemPkgLPr(null);var empty=settings.getDisabledSystemPkgLPr("");
        if(unnamed==null || empty==null || unnamed==empty || unnamed.getPackageName()!=null || !"".equals(empty.getPackageName()) || unnamed.getVersionCode()!=3 || empty.getVersionCode()!=2) throw new AssertionError("original nullable factory identity assumption differs");
        Files.write(new File(directory,"nullable-factory-original.metadata").toPath(),(unnamed.getAppId()+"|"+unnamed.getPathString()+"|"+empty.getAppId()+"|"+empty.getPathString()).getBytes(java.nio.charset.StandardCharsets.UTF_8));
        String writeStatus="ok";
        try {
            var output=new java.io.ByteArrayOutputStream();var serializer=android.util.Xml.resolveSerializer(output);serializer.startDocument(null,true);serializer.startTag(null,"packages");
            settings.writeDisabledSysPackageLPr(serializer,unnamed);serializer.endTag(null,"packages");serializer.endDocument();
            Files.write(new File(directory,"nullable-factory-original.write").toPath(),output.toByteArray());
        } catch(NullPointerException failure) { writeStatus="null-input"; }
        if(!"null-input".equals(writeStatus)) throw new AssertionError("original null factory serialization assumption differs");
        Files.write(new File(directory,"nullable-factory-original.write-status").toPath(),writeStatus.getBytes(java.nio.charset.StandardCharsets.UTF_8));
    }
    private static void verifySharedSeedEvents(File directory) throws Exception {
        String[] names={"android.uid.system","android.uid.phone","android.uid.log","android.uid.nfc","android.uid.bluetooth","android.uid.shell","android.uid.se","android.uid.networkstack","android.uid.uwb","android.uid.vendor.fixture"};
        int[] ids={1000,1001,1007,1027,1002,2000,1031,1073,1083,2901};
        for(int index=0;;index++) {
            var input=new File(directory,"shared-seed-event-"+index+".xml");if(!input.exists())break;
            var data=new File(directory,"shared-seed-original-"+index);var system=new File(data,"system");system.mkdirs();
            Files.write(new File(system,"packages.xml").toPath(),Files.readAllBytes(input.toPath()));
            var settings=new Settings(data,null,null,null,null,new PackageManagerTracedLock());
            for(int at=0;at<ids.length;at++)settings.addSharedUserLPw(names[at],ids[at],1,8);
            var users=new java.util.ArrayList<android.content.pm.UserInfo>();for(int id:new int[]{10,0}){var user=new android.content.pm.UserInfo();user.id=id;users.add(user);}
            settings.readSettingsLPw(null,users,new android.util.ArrayMap<>());
            for(int at=0;at<ids.length;at++) {
                var owner=settings.getSettingLPr(ids[at]);String expected=new String(Files.readAllBytes(new File(directory,"shared-seed-event-"+index+"-"+ids[at]+".metadata").toPath()),java.nio.charset.StandardCharsets.UTF_8);
                if(owner==null || !(names[at]+"|"+owner.getFlags()).equals(expected) || owner.getPrivateFlags()!=8)throw new AssertionError("original seed identity/flags differ "+index+":"+ids[at]);
                byte[] actual=PackageLegacyPermissions.capture(ids[at],new int[]{10,0},owner.getLegacyPermissionState());
                if(!java.util.Arrays.equals(actual,Files.readAllBytes(new File(directory,"shared-seed-event-"+index+"-"+ids[at]+".permissions").toPath())))throw new AssertionError("original seeded permission state differs "+index+":"+ids[at]);
            }
        }
    }
    private static void verifyInitialRestrictions(File directory, java.util.function.BiConsumer<com.android.server.pm.verify.domain.DomainVerificationService, PackageSetting> connect) throws Exception {
        for(int index=0;index<3;index++) {
            var data=new File(directory,"initial-restrictions-native-"+index);
            if(android.os.Looper.myLooper()==null)android.os.Looper.prepareMainLooper();
            var context=android.app.ActivityThread.systemMain().getSystemUiContext();
            var domain=new com.android.server.pm.verify.domain.DomainVerificationService(context,new com.android.server.SystemConfig(false),null);
            var settings=new Settings(data,null,null,domain,null,new PackageManagerTracedLock());
            settings.readSettingsLPw(null,java.util.List.of(),new android.util.ArrayMap<>());
            connect.accept(domain,settings.getPackagesLocked().get("p"));
            settings.readPackageRestrictionsLPr(0,new android.util.ArrayMap<>());
            var state=settings.getPackagesLocked().get("p").readUserState(0);
            String actual=state.isInstalled()+"|"+state.isStopped()+"|"+state.isNotLaunched()+"|"+state.isHidden()+"|"+state.getEnabledState()+"|"+state.getCeDataInode()+"|"+state.getDeDataInode()+"|"+state.getFirstInstallTimeMillis()+"|"+state.getLastDisableAppCaller();
            String expected=new String(Files.readAllBytes(new File(directory,"initial-restrictions-"+index+".expected").toPath()),java.nio.charset.StandardCharsets.UTF_8);
            if(!actual.equals(expected))throw new AssertionError("original native initial restriction read differs "+index+" "+actual+" != "+expected);
            if(index==1 && (!state.isComponentEnabled("p.Enabled") || !state.isComponentDisabled("p.Disabled")))throw new AssertionError("native initial components differ");
            if(index==2 && (!state.isSuspended() || !state.isQuarantined() || state.getArchiveState()==null || state.getSuspendParams().size()!=1))throw new AssertionError("native initial suspension/archive differs");
            if(index==2) {
                var params=state.getSuspendParams().values().iterator().next();
                if(!"Stopped".equals(params.getDialogInfo().getTitle()) || !Integer.valueOf(3).equals(params.getAppExtras().get("count")) || !Long.valueOf(4).equals(((android.os.PersistableBundle)params.getAppExtras().get("nested")).get("time")) || !java.util.Arrays.equals((String[])params.getAppExtras().get("names"),new String[]{"one","two"}) || !Boolean.TRUE.equals(params.getLauncherExtras().get("shown")))throw new AssertionError("native initial suspension extras differ");
                var archive=state.getArchiveState();var activity=archive.getActivityInfos().get(0);
                if(!"Installer".equals(archive.getInstallerTitle()) || archive.getArchiveTimeMillis()!=0x77 || !"Archived".equals(activity.getTitle()) || !"p/p.Main".equals(activity.getOriginalComponentName().flattenToString()) || !"/data/archive/icon.png".equals(activity.getIconBitmap().toString()))throw new AssertionError("native initial archive fields differ");
            }
        }
    }
    private void compareBytes(File root, String stem, int id, LegacyPermissionState state) throws Exception {
        byte[] expected = Files.readAllBytes(new File(root, stem + ".input").toPath());
        byte[] actual = PackageLegacyPermissions.capture(id, users, state);
        if (!java.util.Arrays.equals(expected, actual)) throw new AssertionError("Settings permission restoration differs: " + stem);
    }
    private void readSettings(TypedXmlPullParser parser) throws Exception {
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
            if (event == 3 || event == 4) continue;
            switch (parser.getName()) {
                case "package": readPackage(parser, false); break;
                case "updated-package": readPackage(parser, true); break;
                case "shared-user": readShared(parser); break;
                case "version": case "renamed-package": case "verifier":
                case "last-platform-version": case "database-version":
                case "preferred-packages": case "read-external-storage": break;
                default: skip(parser);
            }
        }
    }
    private void readPackage(TypedXmlPullParser parser, boolean disabled) throws Exception {
        String name = parser.getAttributeValue(null, "name");
        var owner = new PackageSetting(name, null, new File(parser.getAttributeValue(null, "codePath")), 0, 0, new java.util.UUID(0, 0));
        int id = parser.getAttributeInt(null, "userId", 0);
        if (id <= 0) {
            id = parser.getAttributeInt(null, "sharedUserId", 0);
            owner.setSharedUserAppId(id);
        } else if (!disabled) visible.put(id, owner);
        owner.setAppId(id);
        (disabled ? factory : active).put(name, owner);
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
            if (event == 3 || event == 4) continue;
            String tag = parser.getName();
            if ("perms".equals(tag)) {
                SettingBase target = owner.hasSharedUser() ? visible.get(id) : owner;
                if (target != null) {
                    readInstall(parser, target.getLegacyPermissionState());
                    if (!disabled) owner.setInstallPermissionsFixed(true);
                }
            } else if (!disabled && ("proper-signing-keyset".equals(tag) || "signing-keyset".equals(tag)
                    || "upgrade-keyset".equals(tag) || "defined-keyset".equals(tag))) {
                // Attribute-only and obsolete branches do not consume their subtree.
            } else skip(parser);
        }
    }
    private void readShared(TypedXmlPullParser parser) throws Exception {
        String name = parser.getAttributeValue(null, "name");
        int id = parser.getAttributeInt(null, "userId");
        if (!shared.containsKey(name)) seed(name, id);
        var owner = shared.get(name);
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
            if (event == 3 || event == 4) continue;
            if ("perms".equals(parser.getName())) readInstall(parser, owner.getLegacyPermissionState());
            else skip(parser);
        }
    }
    private static void skip(TypedXmlPullParser parser) throws Exception {
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {}
    }
}
