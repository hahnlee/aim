package dev.aim.server;
import android.content.ComponentName;
/** Original Intent serialization only; native calls the actual AM as its inbound caller. */
interface IPackageLaunchBridge {
    byte[] buildLaunchIntent(String category, String targetPackage, in ComponentName component);
}
