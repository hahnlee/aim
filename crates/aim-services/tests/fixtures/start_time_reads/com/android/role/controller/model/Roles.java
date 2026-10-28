package com.android.role.controller.model;
public class Roles {
    public static int calls;
    public static final android.util.ArrayMap<String,Role> roles = new android.util.ArrayMap<>();
    public static android.util.ArrayMap<String,Role> get(android.content.Context context) { calls++; return roles; }
}
