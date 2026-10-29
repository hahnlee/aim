package dev.aim.server;
/** Cold original Animation XML evaluation; native code owns vsync playback. */
interface IActivityAnimations {
    float[] sample(String packageName, int userId, int theme, int style, int attribute,
            int custom, int width, int height);
}
