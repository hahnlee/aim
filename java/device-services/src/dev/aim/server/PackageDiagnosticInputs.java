package dev.aim.server;

import android.os.Binder;
import com.android.internal.util.IndentingPrintWriter;
import com.android.server.LocalManagerRegistry;
import com.android.server.art.ArtManagerLocal;
import com.android.server.pm.PackageManagerLocal;
import java.io.StringWriter;

/** Independent original ART service, using the native retained local facade. */
final class PackageDiagnosticInputs extends IPackageDiagnosticInputs.Stub {
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
