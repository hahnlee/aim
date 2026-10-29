package android.os;
public final class Handler {
    public boolean accepting=true;public final java.util.ArrayDeque<Runnable> queue=new java.util.ArrayDeque<>();
    public boolean post(Runnable task){if(!accepting)return false;queue.add(task);return true;}
    public void drain(){while(!queue.isEmpty())queue.remove().run();}
}
