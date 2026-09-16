package contract;
// Test-only generation of the same typed Map wire shape as IPackageManager.
interface IDexReport {
    oneway void notifyDexLoad(String loadingPackageName,
            in Map<String, String> classLoaderContextMap, String loaderIsa);
}
