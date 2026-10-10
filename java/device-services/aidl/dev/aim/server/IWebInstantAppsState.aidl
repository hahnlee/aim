package dev.aim.server;
import dev.aim.server.IWebInstantAppsChanged;
interface IWebInstantAppsState {
    byte[] capture();
    void start(in IWebInstantAppsChanged callback);
    void stop();
}
