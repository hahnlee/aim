package com.android.media.flags;
public final class Flags {
 public static boolean privileged,mirroring;
 public static boolean enablePrivilegedRoutingForMediaRoutingControl(){return privileged;}
 public static boolean enableMirroringInMediaRouter2(){return mirroring;}
}
