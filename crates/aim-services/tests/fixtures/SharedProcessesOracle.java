package com.android.server.pm;

import com.android.internal.pm.parsing.pkg.PackageImpl;
import com.android.server.pm.parsing.PackageCacher;
import com.android.server.pm.pkg.AndroidPackage;
import java.io.DataOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.file.Files;
import java.nio.charset.StandardCharsets;
import java.util.UUID;

/** Exercise the original shared process owner with native collected code. */
public final class SharedProcessesOracle {
    private static void string(DataOutputStream out, String value) throws Exception {
        byte[] bytes = value == null ? null : value.getBytes(StandardCharsets.UTF_8);
        out.writeInt(bytes == null ? -1 : bytes.length);
        if (bytes != null) out.write(bytes);
    }
    private static void write(File root, String phase, SharedUserSetting group) throws Exception {
        try (var out = new DataOutputStream(new FileOutputStream(new File(root, phase + ".original")))) {
            var processes = group.processes;
            out.writeInt(processes.size());
            for (var entry : processes.entrySet()) {
                string(out, entry.getKey());
                var process = entry.getValue(); string(out, process.getName());
                var classes = process.getAppClassNamesByPackage(); out.writeInt(classes.size());
                for (var item : classes.entrySet()) { string(out, item.getKey()); string(out, item.getValue()); }
                var denied = process.getDeniedPermissions(); out.writeInt(denied.size());
                for (var name : denied) string(out, name);
                out.writeInt(process.getGwpAsanMode()); out.writeInt(process.getMemtagMode());
                out.writeInt(process.getNativeHeapZeroInitialized()); out.writeBoolean(process.isUseEmbeddedDex());
            }
        }
    }
    private static PackageSetting member(File root, String name) throws Exception {
        var code = (PackageImpl)PackageCacher.fromCacheEntryStatic(Files.readAllBytes(new File(root, name + ".cache").toPath()));
        var setting = new PackageSetting(name, null, new File("/data/app/" + name), 0, 0, new UUID(1, 1));
        setting.setPkg((AndroidPackage)(Object)code);
        return setting;
    }
    private static void order(File root, String phase, SharedUserSetting group) throws Exception {
        var entries = group.getPackageStates().toArray();
        try (var out = new DataOutputStream(new FileOutputStream(new File(root, phase + ".order")))) {
            out.writeInt(entries.length);
            for (int i = entries.length - 1; i >= 0; i--) {
                string(out, ((com.android.server.pm.pkg.PackageState)entries[i]).getPackageName());
            }
        }
    }
    public static void main(String[] args) throws Throwable {
        try { execute(args); }
        catch (Throwable failure) { failure.printStackTrace(); throw failure; }
    }
    private static void execute(String[] args) throws Exception {
        File root = new File(args[0]);
        var group = new SharedUserSetting("group", 0, 0);
        var a = member(root, "a"); var b = member(root, "b"); var c = member(root, "c");
        write(root, "empty", group);
        group.addPackage(a); write(root, "a", group);
        group.addPackage(b); write(root, "ab", group);
        group.addProcesses(null); group.addProcesses(java.util.Map.of()); write(root, "noop", group);
        group.addPackage(c); write(root, "abc", group);
        group.updateProcesses(); order(root, "rebuilt", group); write(root, "rebuilt", group);
        if (!group.removePackage(b)) throw new AssertionError("member removal failed");
        order(root, "removed", group); write(root, "removed", group);
        if (group.removePackage(b)) throw new AssertionError("absent removal changed owner");
        write(root, "absent", group);
        group.removePackage(a); group.removePackage(c); write(root, "cleared", group);
        var emptyClassOwner = new SharedUserSetting("empty-class-owner", 0, 0);
        emptyClassOwner.addPackage(member(root, "d"));
        try { emptyClassOwner.addPackage(a); throw new AssertionError("immutable original empty process class map accepted new classes"); }
        catch (UnsupportedOperationException expected) {}
        System.out.println("SHARED_PROCESSES copy union overwrite modes embedded collisions rebuild removal empty");
    }
}
