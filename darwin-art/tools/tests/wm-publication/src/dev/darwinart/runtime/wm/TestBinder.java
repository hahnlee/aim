package dev.darwinart.runtime.wm;

import android.os.IBinder;

final class TestBinder implements IBinder {
    final String name;

    TestBinder(String value) { name = value; }
}
