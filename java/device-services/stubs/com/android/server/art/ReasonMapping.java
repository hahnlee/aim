// Compile-only actual image ABI.
package com.android.server.art;public class ReasonMapping {
    private ReasonMapping() { throw new RuntimeException("stub"); }public static final String REASON_INSTALL="install";public static final String REASON_FIRST_BOOT="first-boot",REASON_BOOT_AFTER_OTA="boot-after-ota",REASON_BOOT_AFTER_MAINLINE_UPDATE="boot-after-mainline-update",REASON_INACTIVE="inactive";}
