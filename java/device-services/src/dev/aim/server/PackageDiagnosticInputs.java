package dev.aim.server;

import android.os.Binder;
import com.android.internal.util.IndentingPrintWriter;
import com.android.server.LocalManagerRegistry;
import com.android.server.art.ArtManagerLocal;
import com.android.server.pm.PackageManagerLocal;
import java.io.StringWriter;

/** Original service diagnostics over the live local package owner. */
final class PackageDiagnosticInputs extends IPackageDiagnosticInputs.Stub {
    private final android.content.Context context;

    PackageDiagnosticInputs(android.content.Context context) {
        this.context = java.util.Objects.requireNonNull(context);
    }

    @Override public String computeRolePackageStateHash(int userId) {
        if (Binder.getCallingUid() != 1000) {
            throw new SecurityException("Package diagnostics requires system uid");
        }
        var packages = com.android.server.LocalServices.getService(
                android.content.pm.PackageManagerInternal.class);
        var users = com.android.server.LocalServices.getService(
                com.android.server.pm.UserManagerInternal.class);
        if (packages == null || users == null) {
            throw new IllegalStateException("Live package/user owner absent");
        }
        if (userId < 0 || users.getUserInfo(userId) == null) {
            throw new IllegalArgumentException("Unknown role diagnostic user " + userId);
        }
        return new com.android.server.policy.role.RoleServicePlatformHelperImpl(context)
                .computePackageStateHash(userId);
    }

    @Override public String dumpDexopt(String packageName) {
        if (Binder.getCallingUid() != 1000) {
            throw new SecurityException("Package diagnostics requires system uid");
        }
        ArtManagerLocal art = LocalManagerRegistry.getManager(ArtManagerLocal.class);
        PackageManagerLocal packages = LocalManagerRegistry.getManager(PackageManagerLocal.class);
        if (art == null || packages == null) {
            throw new IllegalStateException("Independent ART/native package snapshot owner absent");
        }
        StringWriter text = new StringWriter();
        IndentingPrintWriter writer = new IndentingPrintWriter(text, "  ");
        writer.println("Dexopt state:");
        writer.increaseIndent();
        try (PackageManagerLocal.FilteredSnapshot snapshot = packages.withFilteredSnapshot()) {
            if (packageName == null) { art.dump(writer, snapshot); }
            else {
                try { art.dumpPackage(writer, snapshot, packageName); }
                catch (IllegalArgumentException error) { writer.println(error); }
            }
        }
        writer.decreaseIndent();
        writer.flush();
        return text.toString();
    }
}
