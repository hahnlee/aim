package dev.aim.server;
/** Cold original Animation XML evaluation; native code owns vsync playback. */
import android.os.Bundle;
interface IActivityAnimations {
    float[] sample(String packageName, int userId, int theme, int style, int attribute,
            int custom, int width, int height);
    oneway void notifyAnimation(in Bundle options, boolean started);
}
