package dev.aim.server;
/** System-server owned AppsFilter global FeatureConfig changes. */
oneway interface IPackageFilteringUpdates { void changed(boolean disabled); }
