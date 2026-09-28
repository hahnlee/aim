import java.nio.file.Files;
import java.nio.file.Path;
import jdk.internal.org.objectweb.asm.ClassWriter;
import jdk.internal.org.objectweb.asm.Handle;
import jdk.internal.org.objectweb.asm.Label;
import jdk.internal.org.objectweb.asm.MethodVisitor;
import jdk.internal.org.objectweb.asm.Opcodes;
import jdk.internal.org.objectweb.asm.Type;

/** Builds a classfile whose custom bootstrap contract maps directly to DEX invoke-custom. */
public final class GenerateJitInvokeCustom implements Opcodes {
    private static final String OWNER = "dev/aim/probe/JitInvokeCustom";
    private static final String BOOTSTRAP_DESCRIPTOR =
            "(Ljava/lang/invoke/MethodHandles$Lookup;Ljava/lang/String;"
                    + "Ljava/lang/invoke/MethodType;Ljava/lang/invoke/MethodHandle;)"
                    + "Ljava/lang/invoke/CallSite;";

    private static void target(ClassWriter writer, String name, String descriptor) {
        MethodVisitor method = writer.visitMethod(ACC_PRIVATE | ACC_STATIC, name, descriptor, null, null);
        method.visitCode();
        switch (name) {
            case "add17":
                method.visitVarInsn(ILOAD, 0);
                method.visitIntInsn(BIPUSH, 17);
                method.visitInsn(IADD);
                method.visitInsn(IRETURN);
                break;
            case "add":
                method.visitVarInsn(ILOAD, 0);
                method.visitVarInsn(ILOAD, 1);
                method.visitInsn(IADD);
                method.visitInsn(IRETURN);
                break;
            case "identity":
                method.visitVarInsn(ALOAD, 0);
                method.visitInsn(ARETURN);
                break;
            case "checkedTriple":
                Label nonNegative = new Label();
                method.visitVarInsn(ILOAD, 0);
                method.visitJumpInsn(IFGE, nonNegative);
                method.visitTypeInsn(NEW, "java/lang/IllegalArgumentException");
                method.visitInsn(DUP);
                method.visitLdcInsn("negative invoke-custom operand");
                method.visitMethodInsn(INVOKESPECIAL, "java/lang/IllegalArgumentException", "<init>",
                        "(Ljava/lang/String;)V", false);
                method.visitInsn(ATHROW);
                method.visitLabel(nonNegative);
                method.visitVarInsn(ILOAD, 0);
                method.visitInsn(ICONST_3);
                method.visitInsn(IMUL);
                method.visitInsn(IRETURN);
                break;
            case "rangeTarget":
                method.visitVarInsn(LLOAD, 6);
                for (int local = 0; local < 6; ++local) {
                    method.visitVarInsn(ILOAD, local);
                    method.visitInsn(I2L);
                    method.visitInsn(LADD);
                }
                method.visitVarInsn(DLOAD, 8);
                method.visitInsn(D2L);
                method.visitInsn(LADD);
                Label nullReference = new Label();
                method.visitVarInsn(ALOAD, 10);
                method.visitJumpInsn(IFNULL, nullReference);
                method.visitLdcInsn(97L);
                method.visitInsn(LADD);
                method.visitLabel(nullReference);
                method.visitInsn(LRETURN);
                break;
            default:
                throw new AssertionError(name);
        }
        method.visitMaxs(0, 0);
        method.visitEnd();
    }

    private static void wrapper(ClassWriter writer, String name, String descriptor,
            String targetName, String targetDescriptor, int returnOpcode) {
        MethodVisitor method = writer.visitMethod(ACC_PUBLIC | ACC_STATIC, name, descriptor, null, null);
        method.visitCode();
        Type[] arguments = Type.getArgumentTypes(descriptor);
        int local = 0;
        for (Type argument : arguments) {
            method.visitVarInsn(argument.getOpcode(ILOAD), local);
            local += argument.getSize();
        }
        Handle bootstrap = new Handle(H_INVOKESTATIC, OWNER, "bootstrap", BOOTSTRAP_DESCRIPTOR, false);
        Handle target = new Handle(H_INVOKESTATIC, OWNER, targetName, targetDescriptor, false);
        method.visitInvokeDynamicInsn(name, descriptor, bootstrap, target);
        method.visitInsn(returnOpcode);
        method.visitMaxs(0, 0);
        method.visitEnd();
    }

    public static void main(String[] arguments) throws Exception {
        if (arguments.length != 1) throw new IllegalArgumentException("expected class output root");
        ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS);
        writer.visit(V1_8, ACC_PUBLIC | ACC_FINAL | ACC_SUPER, OWNER, null, "java/lang/Object", null);

        MethodVisitor constructor = writer.visitMethod(ACC_PRIVATE, "<init>", "()V", null, null);
        constructor.visitCode();
        constructor.visitVarInsn(ALOAD, 0);
        constructor.visitMethodInsn(INVOKESPECIAL, "java/lang/Object", "<init>", "()V", false);
        constructor.visitInsn(RETURN);
        constructor.visitMaxs(0, 0);
        constructor.visitEnd();

        MethodVisitor bootstrap = writer.visitMethod(
                ACC_PUBLIC | ACC_STATIC, "bootstrap", BOOTSTRAP_DESCRIPTOR, null, null);
        bootstrap.visitCode();
        bootstrap.visitTypeInsn(NEW, "java/lang/invoke/ConstantCallSite");
        bootstrap.visitInsn(DUP);
        bootstrap.visitVarInsn(ALOAD, 3);
        bootstrap.visitVarInsn(ALOAD, 2);
        bootstrap.visitMethodInsn(INVOKEVIRTUAL, "java/lang/invoke/MethodHandle", "asType",
                "(Ljava/lang/invoke/MethodType;)Ljava/lang/invoke/MethodHandle;", false);
        bootstrap.visitMethodInsn(INVOKESPECIAL, "java/lang/invoke/ConstantCallSite", "<init>",
                "(Ljava/lang/invoke/MethodHandle;)V", false);
        bootstrap.visitInsn(ARETURN);
        bootstrap.visitMaxs(0, 0);
        bootstrap.visitEnd();

        target(writer, "add17", "(I)I");
        target(writer, "add", "(II)I");
        target(writer, "identity", "(Ljava/lang/Object;)Ljava/lang/Object;");
        target(writer, "checkedTriple", "(I)I");
        String rangeDescriptor = "(IIIIIIJDLjava/lang/Object;)J";
        target(writer, "rangeTarget", rangeDescriptor);
        wrapper(writer, "stateless", "(I)I", "add17", "(I)I", IRETURN);
        wrapper(writer, "capturing", "(II)I", "add", "(II)I", IRETURN);
        wrapper(writer, "reference", "(Ljava/lang/Object;)Ljava/lang/Object;", "identity",
                "(Ljava/lang/Object;)Ljava/lang/Object;", ARETURN);
        wrapper(writer, "throwing", "(I)I", "checkedTriple", "(I)I", IRETURN);
        wrapper(writer, "range", rangeDescriptor, "rangeTarget", rangeDescriptor, LRETURN);
        writer.visitEnd();

        Path output = Path.of(arguments[0]).resolve(OWNER + ".class");
        Files.createDirectories(output.getParent());
        Files.write(output, writer.toByteArray());
    }
}
