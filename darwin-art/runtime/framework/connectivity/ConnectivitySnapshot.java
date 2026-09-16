package dev.darwinart.runtime.connectivity;

/** Immutable host facts used to build an Android connectivity projection. */
public final class ConnectivitySnapshot {
    private final boolean pathSatisfied;
    private final boolean expensive;
    private final boolean constrained;
    private final boolean validated;
    private final int interfaceMask;
    private final long generation;
    private final String interfaceName;
    private final String[] dnsServers;

    private ConnectivitySnapshot(boolean satisfied, boolean isExpensive, boolean isConstrained,
            boolean isValidated,
            int observedInterfaceMask, long observedGeneration, String observedInterfaceName,
            String[] observedDnsServers) {
        pathSatisfied = satisfied;
        expensive = isExpensive;
        constrained = isConstrained;
        validated = satisfied && isValidated;
        interfaceMask = observedInterfaceMask;
        generation = observedGeneration;
        interfaceName = observedInterfaceName;
        dnsServers = observedDnsServers == null
                ? new String[0] : observedDnsServers.clone();
    }

    public static ConnectivitySnapshot fromNetworkPath(boolean satisfied, boolean expensive,
            boolean constrained, int interfaceMask) {
        return fromNetworkPath(satisfied, expensive, constrained, interfaceMask, 0, null, null);
    }

    /** Builds a snapshot with copied, numeric default DNS and primary-interface facts. */
    public static ConnectivitySnapshot fromNetworkPath(boolean satisfied, boolean expensive,
            boolean constrained, int interfaceMask, long generation, String interfaceName,
            String[] dnsServers) {
        return new ConnectivitySnapshot(satisfied, expensive, constrained, false, interfaceMask,
                generation, interfaceName, dnsServers);
    }

    /** A conservative snapshot for states which have no host path yet. */
    public static ConnectivitySnapshot unavailable() {
        return new ConnectivitySnapshot(false, false, false, false, 0, 0, null, null);
    }

    /** Compatibility constructor for injected state tests and simple providers. */
    public static ConnectivitySnapshot fromMetered(boolean metered) {
        return new ConnectivitySnapshot(true, metered, false, false, 0, 0, null, null);
    }

    /** Returns the same copied path facts with an Android-owned validation result. */
    public ConnectivitySnapshot withValidation(boolean value) {
        return new ConnectivitySnapshot(pathSatisfied, expensive, constrained, value,
                interfaceMask, generation, interfaceName, dnsServers);
    }

    public boolean hasActiveNetwork() { return pathSatisfied; }

    /** Android meteredness is conservative when no default path exists. */
    public boolean isMetered() { return !pathSatisfied || expensive; }

    public boolean isConstrained() { return constrained; }

    /** True only after the Android connectivity owner validates this path generation. */
    public boolean isValidated() { return validated; }

    public int interfaceMask() { return interfaceMask; }

    /** Monotonic provider generation; increments when any copied host fact changes. */
    public long generation() { return generation; }

    /** Primary interface selected by macOS for the global default IPv4 service. */
    public String interfaceName() { return interfaceName; }

    /** Returns a defensive copy of numeric global default DNS server addresses. */
    public String[] dnsServers() { return dnsServers.clone(); }
}
