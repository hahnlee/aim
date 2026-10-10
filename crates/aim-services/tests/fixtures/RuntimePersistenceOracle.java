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
        Files.write(file.toPath(),Files.readAllBytes(new File(directory,"duplicate-runtime-permissions.xml").toPath()));
        var duplicate=persistence.readForUser(user);
        if(!new java.util.ArrayList<>(duplicate.getPackagePermissions().keySet()).equals(List.of("z","BB","Aa")) || duplicate.getPackagePermissions().get("BB").size()!=1 || !"last".equals(duplicate.getPackagePermissions().get("BB").get(0).getName()) || duplicate.getPackagePermissions().get("BB").get(0).getFlags()!=2 || !duplicate.getSharedUserPermissions().get("group").isEmpty())throw new AssertionError("runtime duplicate map replacement/order differs");
        persistence.writeForUser(duplicate,user);
        Files.write(new File(directory,"original-duplicate-runtime-permissions.xml").toPath(),Files.readAllBytes(file.toPath()));
        var nullOwners=Files.readAllBytes(new File(directory,"nullable-runtime-owners.xml").toPath());
        Files.write(file.toPath(),nullOwners);
        var nullable=persistence.readForUser(user);
        var keys=new java.util.ArrayList<>(nullable.getPackagePermissions().keySet());
        if(keys.size()!=2 || keys.get(0)!=null || !"".equals(keys.get(1)) || !"last".equals(nullable.getPackagePermissions().get(null).get(0).getName()) || !nullable.getSharedUserPermissions().containsKey(null) || !nullable.getSharedUserPermissions().containsKey(""))throw new AssertionError("nullable runtime owner identities differ");
        persistence.writeForUser(nullable,user);
        if(!java.util.Arrays.equals(nullOwners,Files.readAllBytes(file.toPath())))throw new AssertionError("original null-owner write did not restore its file");
        var nullPermission=Files.readAllBytes(new File(directory,"nullable-runtime-permission.xml").toPath());
        Files.write(file.toPath(),nullPermission);
        nullable=persistence.readForUser(user);
        if(nullable.getPackagePermissions().get("p").get(0).getName()!=null)throw new AssertionError("null runtime permission became a string");
        persistence.writeForUser(nullable,user);
        if(!java.util.Arrays.equals(nullPermission,Files.readAllBytes(file.toPath())))throw new AssertionError("original null-permission write did not restore its file");
    }
    private static void check(RuntimePermissionsState state) {
        if(state==null || state.getVersion()!=7 || !"finger<&\"\t\n\r".equals(state.getFingerprint()))throw new AssertionError("runtime version/fingerprint differs");
        var permissions=state.getPackagePermissions().get("p");
        if(permissions==null || permissions.size()!=2 || !"ordinary".equals(permissions.get(0).getName()) || !permissions.get(0).isGranted() || permissions.get(0).getFlags()!=17 || !"one-time".equals(permissions.get(1).getName()) || permissions.get(1).isGranted() || permissions.get(1).getFlags()!=(1<<16))throw new AssertionError("runtime one-time/ordinary state differs");
        if(state.getSharedUserPermissions().size()!=1 || !state.getSharedUserPermissions().get("group").isEmpty())throw new AssertionError("empty runtime shared owner differs");
    }
}
