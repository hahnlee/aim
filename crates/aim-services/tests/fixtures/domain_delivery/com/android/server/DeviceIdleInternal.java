package com.android.server;
public final class DeviceIdleInternal {
    public record Allowlist(int uid,String name,long duration,int user,boolean sync,int reason,String label) {}
    public final java.util.List<Allowlist> calls=new java.util.ArrayList<>();
    public void addPowerSaveTempWhitelistApp(int uid,String name,long duration,int user,boolean sync,int reason,String label){calls.add(new Allowlist(uid,name,duration,user,sync,reason,label));}
}
