// Compile-only pinned image API; no runtime implementation.
package com.android.server.pm;
public class DumpState {
    public boolean isDumping(int type) { throw new RuntimeException("stub"); }
    public boolean isOptionEnabled(int option) { throw new RuntimeException("stub"); }
    public boolean getTitlePrinted() { throw new RuntimeException("stub"); }
    public void setTitlePrinted(boolean value) { throw new RuntimeException("stub"); }
    public SharedUserSetting getSharedUser() { throw new RuntimeException("stub"); }
    public String getTargetPackageName() { throw new RuntimeException("stub"); }
    public boolean isFullPreferred() { throw new RuntimeException("stub"); }
    public boolean isCheckIn() { throw new RuntimeException("stub"); }
    public boolean isBrief() { throw new RuntimeException("stub"); }
}
