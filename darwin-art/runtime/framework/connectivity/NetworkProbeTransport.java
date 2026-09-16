package dev.darwinart.runtime.connectivity;

/** Executes one Internet-validation observation without owning Android policy. */
interface NetworkProbeTransport {
    enum Result {
        VALIDATED,
        CAPTIVE_PORTAL,
        FAILED
    }

    Result probe();
}
