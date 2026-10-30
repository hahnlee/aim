package dev.aim.server;

import android.content.Context;
import android.content.pm.PackageManager;
import android.media.AudioAttributes;
import android.media.AudioDeviceAttributes;
import android.media.AudioSystem;
import android.media.Utils;
import android.media.audiopolicy.AudioProductStrategy;
import android.os.Binder;
import android.os.Parcel;
import android.os.Process;
import java.util.ArrayList;

/** AudioService.getAudioProductStrategies, android-16.0.0_r1, after its native
 * endpoint's permission check. The original AudioSystem queries audioserver's
 * policy engine, including the image configuration and its default strategies.
 * Copyright (C) The Android Open Source Project, Apache License 2.0. */
public final class AudioPolicyQueries extends IAudioPolicyQueries.Stub {
    private final Context context;

    public AudioPolicyQueries(Context context) { this.context = context; }

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

    /** AudioService.getDevicesForAttributes and anonymizeAudioDeviceAttributesList. */
    @Override public byte[] getDevicesForAttributes(AudioAttributes attributes, int callingUid) {
        Bridge.enforceSystemUid();
        long token = Binder.clearCallingIdentity();
        Parcel out = Parcel.obtain();
        try {
            var devices = AudioSystem.getDevicesForAttributes(
                    java.util.Objects.requireNonNull(attributes), false);
            boolean privileged = context.checkPermission(
                    android.Manifest.permission.BLUETOOTH_CONNECT, -1, callingUid)
                    == PackageManager.PERMISSION_GRANTED || callingUid == Process.SYSTEM_UID;
            if (!privileged) {
                var anonymized = new ArrayList<AudioDeviceAttributes>();
                for (var device : devices) {
                    if (device != null && AudioSystem.isBluetoothDevice(device.getInternalType())) {
                        var copy = new AudioDeviceAttributes(device);
                        copy.setAddress(Utils.anonymizeBluetoothAddress(device.getAddress()));
                        anonymized.add(copy);
                    } else {
                        anonymized.add(device);
                    }
                }
                devices = anonymized;
            }
            out.writeTypedList(devices);
            return out.marshall();
        } finally {
            out.recycle();
            Binder.restoreCallingIdentity(token);
        }
    }
}
