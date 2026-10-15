package dev.aim.server;
import android.content.pm.StagedApexInfo;
/** Native staging owner passes its verified ready session to the original apexd owner. */
interface INativeStagingBridge {
    StagedApexInfo[] getStagedApexInfos(int sessionId, in int[] apexChildSessionIds);
}
