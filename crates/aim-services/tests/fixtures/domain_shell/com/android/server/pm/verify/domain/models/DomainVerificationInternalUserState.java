package com.android.server.pm.verify.domain.models;
public record DomainVerificationInternalUserState(int getUserId,android.util.ArraySet<String> getEnabledHosts,boolean isLinkHandlingAllowed) {}
