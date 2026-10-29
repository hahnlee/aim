package android.graphics;
public class GraphicsStatsService extends android.os.Binder {
    public static final String GRAPHICS_STATS_SERVICE = "graphicsstats";
    public GraphicsStatsService(android.content.Context context) {
        dev.aim.server.SystemProcessHostTest.events.add("graphicsstats:construct");
    }
}
