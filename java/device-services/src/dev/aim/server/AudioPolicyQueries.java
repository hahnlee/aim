package dev.aim.server;

import android.media.audiopolicy.AudioProductStrategy;
import android.os.Binder;
import android.os.Parcel;

/** AudioService.getAudioProductStrategies, android-16.0.0_r1, after its native
 * endpoint's permission check. The original AudioSystem queries audioserver's
 * policy engine, including the image configuration and its default strategies.
 * Copyright (C) The Android Open Source Project, Apache License 2.0. */
public final class AudioPolicyQueries extends IAudioPolicyQueries.Stub {
    @Override public byte[] getAudioProductStrategies() {
        Bridge.enforceSystemUid();
        long token = Binder.clearCallingIdentity();
        Parcel out = Parcel.obtain();
        try {
            out.writeTypedList(AudioProductStrategy.getAudioProductStrategies());
            return out.marshall();
        } finally {
            out.recycle();
            Binder.restoreCallingIdentity(token);
        }
    }
}
