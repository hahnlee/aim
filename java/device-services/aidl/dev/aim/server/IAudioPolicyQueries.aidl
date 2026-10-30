package dev.aim.server;

/** Original audio policy configuration parcelables; system UID only. */
import android.media.AudioAttributes;

interface IAudioPolicyQueries {
    byte[] getAudioProductStrategies();
    byte[] getDevicesForAttributes(in AudioAttributes attributes, int callingUid);
}
