package dev.aim.server;

/** Opaque identity lease for an owned preferred resolver record. */
interface IPackageResolverIdentity {
    int getIdentityHash();
}
