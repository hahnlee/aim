package dev.darwinart.runtime.audio;

import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;

/** System-process owner for the Android 16 IAudioService sound-effect calls. */
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
        // Deliberately leave unrelated IAudioService operations unsupported.
        return super.onTransact(code, data, reply, flags);
    }
}
