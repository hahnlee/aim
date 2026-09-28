package dev.aim.runtime.wm;

import android.os.IBinder;

final class TestBinder implements IBinder {
    final String name;

    TestBinder(String value) { name = value; }
}
