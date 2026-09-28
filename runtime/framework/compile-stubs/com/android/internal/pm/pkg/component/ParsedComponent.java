package com.android.internal.pm.pkg.component;

import java.util.List;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar/services.jar. */
public interface ParsedComponent {
    String getName();
    List<ParsedIntentInfo> getIntents();
}
