package android.media;

import android.content.Context;

/**
 * Process-local audio service used when the Darwin runtime has no Android
 * system_server. UI sound effects must remain harmless, while the framework
 * still receives a correctly typed AudioManager from Context.
 */
public final class ProbeAudioManager extends AudioManager {
    private static final int MAX_VOLUME = 15;
    private int musicVolume = 10;

    public ProbeAudioManager(Context context) {
        // The public SDK surface exposes only the no-argument constructor;
        // the host intentionally has no ContextImpl-backed audio service.
        super();
    }

    // This public framework method is hidden from the SDK stub used by javac,
    // so intentionally omit @Override. It overrides the real Android 16
    // virtual method when this class is loaded by ART.
    public boolean areNavigationRepeatSoundEffectsEnabled() {
        return false;
    }

    @Override
    public void playSoundEffect(int effectType) {}

    @Override
    public void playSoundEffect(int effectType, float volume) {}

    @Override
    public int getStreamMaxVolume(int streamType) {
        return MAX_VOLUME;
    }

    @Override
    public int getStreamMinVolume(int streamType) {
        return 0;
    }

    @Override
    public int getStreamVolume(int streamType) {
        return musicVolume;
    }

    @Override
    public void setStreamVolume(int streamType, int index, int flags) {
        musicVolume = Math.max(0, Math.min(MAX_VOLUME, index));
    }

    @Override
    public void adjustStreamVolume(int streamType, int direction, int flags) {
        if (direction > 0) setStreamVolume(streamType, musicVolume + 1, flags);
        if (direction < 0) setStreamVolume(streamType, musicVolume - 1, flags);
    }

    @Override
    public boolean isVolumeFixed() {
        return false;
    }

    @Override
    public boolean isMusicActive() {
        return false;
    }

    @Override
    public int requestAudioFocus(OnAudioFocusChangeListener listener,
            int streamType, int durationHint) {
        return AUDIOFOCUS_REQUEST_GRANTED;
    }

    @Override
    public int requestAudioFocus(AudioFocusRequest focusRequest) {
        return AUDIOFOCUS_REQUEST_GRANTED;
    }

    @Override
    public int abandonAudioFocus(OnAudioFocusChangeListener listener) {
        return AUDIOFOCUS_REQUEST_GRANTED;
    }

    @Override
    public int abandonAudioFocusRequest(AudioFocusRequest focusRequest) {
        return AUDIOFOCUS_REQUEST_GRANTED;
    }
}
