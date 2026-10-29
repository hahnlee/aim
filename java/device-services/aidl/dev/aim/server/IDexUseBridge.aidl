package dev.aim.server;

/** Original ART dex-use owner in aim-system. Authenticated native owner only. */
interface IDexUseBridge {
    void notifyDexLoad(String packageName, in String[] paths, in String[] contexts,
            String loaderIsa, int callingUid);
}
