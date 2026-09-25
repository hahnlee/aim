package dev.darwinart.runtime.audio;

import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;

/**
 * System-process owner for the Android 16 IAudioService sound-effect, stream
 * volume and player-registration calls.
 *
 * <p>Output loudness is owned by the macOS output device, so this service uses
 * AOSP's fixed-volume policy ({@code config_useFixedVolume}): every stream
 * reports its maximum index and volume changes are ignored, exactly as
 * AudioService does when an external sink controls volume.</p>
 */
public final class AudioServiceEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.media.IAudioService";
    private static final int PLAY_SOUND_EFFECT = transaction("playSoundEffect");
    private static final int PLAY_SOUND_EFFECT_VOLUME = transaction("playSoundEffectVolume");
    private static final int LOAD_SOUND_EFFECTS = transaction("loadSoundEffects");
    private static final int UNLOAD_SOUND_EFFECTS = transaction("unloadSoundEffects");
    private static final int RELOAD_AUDIO_SETTINGS = transaction("reloadAudioSettings");
    private static final int NAVIGATION_REPEAT_ENABLED =
            transaction("areNavigationRepeatSoundEffectsEnabled");
    private static final int SET_NAVIGATION_REPEAT_ENABLED =
            transaction("setNavigationRepeatSoundEffectsEnabled");
    private static final int HOME_SOUND_EFFECT_ENABLED = transaction("isHomeSoundEffectEnabled");
    private static final int SET_HOME_SOUND_EFFECT_ENABLED =
            transaction("setHomeSoundEffectEnabled");
    private static final int GET_STREAM_VOLUME = transaction("getStreamVolume");
    private static final int GET_STREAM_MAX_VOLUME = transaction("getStreamMaxVolume");
    private static final int GET_STREAM_MIN_VOLUME = transaction("getStreamMinVolume");
    private static final int GET_LAST_AUDIBLE_STREAM_VOLUME =
            transaction("getLastAudibleStreamVolume");
    private static final int IS_STREAM_MUTE = transaction("isStreamMute");
    private static final int IS_VOLUME_FIXED = transaction("isVolumeFixed");
    private static final int SET_STREAM_VOLUME = transaction("setStreamVolume");
    private static final int ADJUST_STREAM_VOLUME = transaction("adjustStreamVolume");
    private static final int TRACK_PLAYER = transaction("trackPlayer");
    private static final int PLAYER_EVENT = transaction("playerEvent");
    private static final int PLAYER_ATTRIBUTES = transaction("playerAttributes");
    private static final int PLAYER_SESSION_ID = transaction("playerSessionId");
    private static final int RELEASE_PLAYER = transaction("releasePlayer");
    // AudioService.MAX_STREAM_VOLUME / MIN_STREAM_VOLUME, indexed by stream
    // type (VOICE_CALL .. ASSISTANT).
    private static final int[] MAX_STREAM_VOLUME = {5, 7, 7, 15, 7, 7, 15, 7, 15, 15, 15, 15};
    private static final int[] MIN_STREAM_VOLUME = {1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0};
    private final java.util.concurrent.atomic.AtomicInteger nextPlayerId =
            new java.util.concurrent.atomic.AtomicInteger(1);
    private boolean navigationRepeatSoundEffectsEnabled;
    private boolean homeSoundEffectEnabled;

    public AudioServiceEndpoint() { attachInterface(null, DESCRIPTOR); }

    private static int transaction(String name) {
        try {
            java.lang.reflect.Field field = Class.forName("android.media.IAudioService$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == PLAY_SOUND_EFFECT) {
            data.enforceInterface(DESCRIPTOR);
            data.readInt(); // effectType
            data.readInt(); // userId
            data.enforceNoDataAvail();
            return true;
        }
        if (code == PLAY_SOUND_EFFECT_VOLUME) {
            data.enforceInterface(DESCRIPTOR);
            data.readInt(); // effectType
            data.readFloat(); // volume
            data.enforceNoDataAvail();
            return true;
        }
        if (code == LOAD_SOUND_EFFECTS) {
            data.enforceInterface(DESCRIPTOR);
            data.enforceNoDataAvail();
            if (reply == null) return false;
            reply.writeNoException();
            // This host has no Android SoundPool, so report that no samples loaded.
            reply.writeBoolean(false);
            return true;
        }
        if (code == UNLOAD_SOUND_EFFECTS || code == RELOAD_AUDIO_SETTINGS) {
            data.enforceInterface(DESCRIPTOR);
            data.enforceNoDataAvail();
            return true;
        }
        if (code == NAVIGATION_REPEAT_ENABLED || code == HOME_SOUND_EFFECT_ENABLED) {
            data.enforceInterface(DESCRIPTOR);
            data.enforceNoDataAvail();
            if (reply == null) return false;
            reply.writeNoException();
            reply.writeBoolean(code == NAVIGATION_REPEAT_ENABLED
                    ? navigationRepeatSoundEffectsEnabled : homeSoundEffectEnabled);
            return true;
        }
        if (code == SET_NAVIGATION_REPEAT_ENABLED || code == SET_HOME_SOUND_EFFECT_ENABLED) {
            data.enforceInterface(DESCRIPTOR);
            boolean enabled = data.readBoolean();
            data.enforceNoDataAvail();
            if (code == SET_NAVIGATION_REPEAT_ENABLED) {
                navigationRepeatSoundEffectsEnabled = enabled;
            } else {
                homeSoundEffectEnabled = enabled;
            }
            return true;
        }
        if (code == GET_STREAM_VOLUME || code == GET_STREAM_MAX_VOLUME
                || code == GET_STREAM_MIN_VOLUME || code == GET_LAST_AUDIBLE_STREAM_VOLUME
                || code == IS_STREAM_MUTE) {
            data.enforceInterface(DESCRIPTOR);
            int streamType = data.readInt();
            data.enforceNoDataAvail();
            if (reply == null) return false;
            if (!validStreamType(streamType)) {
                reply.writeException(new IllegalArgumentException("Bad stream type " + streamType));
                return true;
            }
            reply.writeNoException();
            if (code == IS_STREAM_MUTE) {
                reply.writeBoolean(false);
            } else {
                // Fixed volume keeps every index at its maximum.
                reply.writeInt(code == GET_STREAM_MIN_VOLUME
                        ? MIN_STREAM_VOLUME[streamType] : MAX_STREAM_VOLUME[streamType]);
            }
            return true;
        }
        if (code == IS_VOLUME_FIXED) {
            data.enforceInterface(DESCRIPTOR);
            data.enforceNoDataAvail();
            if (reply == null) return false;
            reply.writeNoException();
            reply.writeBoolean(true);
            return true;
        }
        if (code == SET_STREAM_VOLUME || code == ADJUST_STREAM_VOLUME) {
            data.enforceInterface(DESCRIPTOR);
            int streamType = data.readInt();
            data.readInt(); // index or direction
            data.readInt(); // flags
            data.readString(); // calling package
            data.enforceNoDataAvail();
            if (reply == null) return true;
            if (!validStreamType(streamType)) {
                reply.writeException(new IllegalArgumentException("Bad stream type " + streamType));
                return true;
            }
            // AudioService returns early for fixed-volume devices.
            reply.writeNoException();
            return true;
        }
        if (code == TRACK_PLAYER) {
            // Player identity for PlayerBase; no playback-activity consumer
            // exists yet, so the PlayerIdCard itself is not retained.
            data.enforceInterface(DESCRIPTOR);
            if (reply == null) return false;
            reply.writeNoException();
            reply.writeInt(nextPlayerId.getAndIncrement());
            return true;
        }
        if (code == PLAYER_EVENT || code == PLAYER_ATTRIBUTES || code == PLAYER_SESSION_ID
                || code == RELEASE_PLAYER) {
            data.enforceInterface(DESCRIPTOR);
            if (reply != null) reply.writeNoException();
            return true;
        }
        // Deliberately leave unrelated IAudioService operations unsupported.
        return super.onTransact(code, data, reply, flags);
    }

    private static boolean validStreamType(int streamType) {
        return streamType >= 0 && streamType < MAX_STREAM_VOLUME.length;
    }
}
