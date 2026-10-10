package dev.aim.server;
/** The actual original ART result, without substituting profile validation. */
parcelable InstallDexoptResult {
    int finalStatus;
    @nullable String[] externalProfileErrors;
    boolean hasResult;
    @nullable String exceptionClass;
    @nullable String exceptionMessage;
}
