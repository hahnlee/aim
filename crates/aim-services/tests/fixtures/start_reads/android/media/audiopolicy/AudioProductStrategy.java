package android.media.audiopolicy;
public class AudioProductStrategy {
    public static int stream;
    public static final android.media.AudioAttributes attributes = null;
    public static android.media.AudioAttributes getAudioAttributesForStrategyWithLegacyStreamType(int s) { stream = s; return attributes; }
    public static java.util.List<AudioProductStrategy> strategies = new java.util.ArrayList<>();
    public static java.util.List<AudioProductStrategy> getAudioProductStrategies() { return strategies; }
}
