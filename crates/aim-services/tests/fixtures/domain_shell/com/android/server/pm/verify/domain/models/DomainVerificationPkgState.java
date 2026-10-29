package com.android.server.pm.verify.domain.models;
public record DomainVerificationPkgState(String getPackageName,java.util.UUID getId,boolean auto,android.util.ArrayMap<String,Integer> getStateMap,android.util.SparseArray<DomainVerificationInternalUserState> getUserStates,String signature) {}
