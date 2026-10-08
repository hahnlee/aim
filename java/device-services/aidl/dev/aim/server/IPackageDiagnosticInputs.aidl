package dev.aim.server;
/** Independent ART diagnostics over native PackageManagerLocal snapshots. */
interface IPackageDiagnosticInputs {
    String dumpDexopt(String packageName);
}
