package dev.aim.server;
final class ArtOwnerStartup {
    static void start(android.content.Context context, Object epoch) {
        if (!SystemProcessHostTest.events.get(SystemProcessHostTest.events.size()-1).equals("attachSystem")) throw new AssertionError("ART before process attachment");
        SystemProcessHostTest.events.add("artAttached");
    }
}
