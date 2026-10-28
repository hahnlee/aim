package dev.aim.server;
final class TelecomLoader {
    TelecomLoader(android.content.Context context) {}
    void onActivityManagerReady() { SystemProcessHostTest.events.add("telecom"); }
}
