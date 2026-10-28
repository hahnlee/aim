package com.android.role.controller.model;
public class Role {
    private Role(String name, boolean bypass, RoleBehavior behavior, String defaults,
            int description, int exclusivity, boolean fallback,
            java.util.function.Supplier<Boolean> flag, int label, int maxSdk, int minSdk,
            boolean onlyAdded, boolean overrideUser, int requestDescription, int requestTitle,
            boolean requestable, int keywords, int shortLabel, boolean showNone, boolean statik,
            boolean systemOnly, boolean visible, java.util.List<?> components,
            java.util.List<?> permissions, java.util.List<?> appOpPermissions,
            java.util.List<?> appOps, java.util.List<?> preferred, String uiBehavior) {}
    public int getExclusivity() { throw new RuntimeException("stub"); }
}
