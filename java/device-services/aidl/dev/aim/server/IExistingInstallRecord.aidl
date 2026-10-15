package dev.aim.server;
/** Revocable native existing-install inputs, streamed within Binder parcel limits. */
interface IExistingInstallRecord {
    int getLength();
    byte[] getChunk(int offset, int length);
    void close();
}
