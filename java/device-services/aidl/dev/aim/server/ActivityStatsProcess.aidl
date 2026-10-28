package dev.aim.server;
parcelable ActivityStatsProcess {
    int uid;
    int pid;
    String processName;
    int oomAdj;
    int procState;
    boolean hasForegroundServices;
    int hostingComponentTypes;
    int historicalHostingComponentTypes;
}
