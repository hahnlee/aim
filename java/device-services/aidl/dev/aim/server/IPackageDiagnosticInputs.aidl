package dev.aim.server;
/** Read-only diagnostics from the original ART and role services. */
interface IPackageDiagnosticInputs {
    String dumpDexopt(String packageName);
    String computeRolePackageStateHash(int userId);
}
