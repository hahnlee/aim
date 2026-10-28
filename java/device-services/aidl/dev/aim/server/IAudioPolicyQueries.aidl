package dev.aim.server;

/** Original audio policy configuration parcelables; system UID only. */
interface IAudioPolicyQueries {
    byte[] getAudioProductStrategies();
}
