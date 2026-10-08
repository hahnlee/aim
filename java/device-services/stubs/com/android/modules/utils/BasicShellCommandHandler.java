// Compile-only declarations verified against the pinned image DEX.
package com.android.modules.utils;
public abstract class BasicShellCommandHandler {
    public BasicShellCommandHandler(){throw new RuntimeException("stub");}
    public abstract int onCommand(String command);
    public abstract void onHelp();
    public String getNextArgRequired(){throw new RuntimeException("stub");}
}
