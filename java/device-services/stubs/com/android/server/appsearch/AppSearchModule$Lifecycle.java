// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
// The image's service-appsearch.jar keeps AppSearchModule's Lifecycle but
// not the outer class, so it is stubbed under its binary name.
package com.android.server.appsearch;

import android.content.Context;

import com.android.server.SystemService;

public class AppSearchModule$Lifecycle extends SystemService {
    public AppSearchModule$Lifecycle(Context context) { super(context); throw new RuntimeException("stub"); }
    @Override public void onStart() { throw new RuntimeException("stub"); }
}
