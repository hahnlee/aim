package android.net;
public final class Network { public final int id; public Network(int id){this.id=id;} @Override public boolean equals(Object value){return value instanceof Network && id==((Network)value).id;} @Override public int hashCode(){return id;} }
