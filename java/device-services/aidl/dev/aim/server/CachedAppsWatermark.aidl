package dev.aim.server;
parcelable CachedAppsWatermark {
    int count;
    int uptimeSeconds;
    int binderProxies;
    int freeKb;
    int cachedKb;
    int zramKb;
    int kernelKb;
    int frozenApps;
    int longestFrozenSeconds;
    int shortestFrozenSeconds;
    int medianFrozenSeconds;
    int averageFrozenSeconds;
}
