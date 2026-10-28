package dev.aim.server;

import android.os.Parcel;

/** ContentValues.getAsString, android-16.0.0_r1; original libcore toString.
 * Copyright (C) The Android Open Source Project, Apache License 2.0. */
public final class SettingsValues extends ISettingsValues.Stub {
    @Override
    public String getAsString(byte[] bytes) {
        Bridge.enforceSystemUid();
        Parcel parcel = Parcel.obtain();
        try {
            parcel.unmarshall(bytes, 0, bytes.length);
            parcel.setDataPosition(0);
            Object value;
            switch (parcel.readInt()) {
                case 7: value = parcel.readFloat(); break;
                case 8: value = parcel.readDouble(); break;
                case 13: value = parcel.createByteArray(); break;
                default: throw new IllegalArgumentException("not a settings scalar");
            }
            parcel.enforceNoDataAvail();
            return asString(value);
        } finally {
            parcel.recycle();
        }
    }

    // ContentValues.getAsString after its map lookup.
    static String asString(Object value) {
        return value != null ? value.toString() : null;
    }
}
