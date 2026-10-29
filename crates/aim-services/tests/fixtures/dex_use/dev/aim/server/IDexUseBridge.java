package dev.aim.server;
public interface IDexUseBridge {
 void notifyDexLoad(String name,String[] paths,String[] contexts,String isa,int uid);
 abstract class Stub implements IDexUseBridge {}
}
