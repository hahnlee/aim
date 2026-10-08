package dev.aim.server;
/** Original ART service only. Native owner retains exact verified session data. */
interface IInstallerCompletionBridge {
    int dexoptInstalled(String packageName,int installScenario,int installReason,int installFlags,
        String compilerFilter,boolean debuggable,boolean instantApp,boolean apex,boolean rollbackFromPlatform);
}
