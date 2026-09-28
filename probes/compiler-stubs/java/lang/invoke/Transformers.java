package java.lang.invoke;

import dalvik.system.EmulatedStackFrame;

/** Signature-only compiler input for Android hidden MethodHandle APIs. */
public final class Transformers {
  private Transformers() {}

  public abstract static class Transformer extends MethodHandle {
    protected Transformer(MethodType type) {
      super();
    }

    public abstract void transform(EmulatedStackFrame frame) throws Throwable;

    protected static void invokeFromTransform(
        MethodHandle target, EmulatedStackFrame frame) throws Throwable {}
  }
}
