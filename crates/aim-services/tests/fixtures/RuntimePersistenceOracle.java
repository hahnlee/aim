package com.android.permission.persistence;
import java.io.File;
import java.nio.file.Files;
import java.util.List;
import java.util.Map;

public final class RuntimePersistenceOracle {
    public static void verify(File directory) throws Exception {
        var user=android.os.UserHandle.of(42);
        var file=RuntimePermissionsPersistenceImpl.getFile(user);
        file.getParentFile().mkdirs();
        var persistence=new RuntimePermissionsPersistenceImpl();
        Files.write(file.toPath(), Files.readAllBytes(new File(directory,"native-runtime-permissions.xml").toPath()));
        check(persistence.readForUser(user));
        var original=new RuntimePermissionsState(7,"finger<&\"\t\n\r",Map.of("p",List.of(
                new RuntimePermissionsState.PermissionState("ordinary",true,17),
                new RuntimePermissionsState.PermissionState("one-time",true,1<<16))),Map.of("group",List.of()));
        persistence.writeForUser(original,user);
        check(persistence.readForUser(user));
        Files.write(new File(directory,"original-runtime-permissions.xml").toPath(),Files.readAllBytes(file.toPath()));
    }
    private static void check(RuntimePermissionsState state) {
        if(state==null || state.getVersion()!=7 || !"finger<&\"\t\n\r".equals(state.getFingerprint()))throw new AssertionError("runtime version/fingerprint differs");
        var permissions=state.getPackagePermissions().get("p");
        if(permissions==null || permissions.size()!=2 || !"ordinary".equals(permissions.get(0).getName()) || !permissions.get(0).isGranted() || permissions.get(0).getFlags()!=17 || !"one-time".equals(permissions.get(1).getName()) || permissions.get(1).isGranted() || permissions.get(1).getFlags()!=(1<<16))throw new AssertionError("runtime one-time/ordinary state differs");
        if(state.getSharedUserPermissions().size()!=1 || !state.getSharedUserPermissions().get("group").isEmpty())throw new AssertionError("empty runtime shared owner differs");
    }
}
