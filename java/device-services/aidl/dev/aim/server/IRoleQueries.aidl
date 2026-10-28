package dev.aim.server;

/** Original RoleService definition checks; callable only by the native system owner. */
interface IRoleQueries {
    void enforceProfileGroupExclusiveRole(String roleName);
}
