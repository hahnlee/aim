package com.android.role;
/** Compile-only Android 16 r1 API; the image supplies the implementation. */
public class RoleService extends com.android.server.SystemService {
    public RoleService(android.content.Context context) { super(context); }
    public void onStart() { throw new RuntimeException("stub"); }
}
