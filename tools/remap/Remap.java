// Remaps an obfuscated Minecraft jar (1.21.11 and older) to Mojang's names with the ProGuard mapping
// file Mojang publishes beside it (`downloads.client_mappings` / `server_mappings` of the version JSON).
// Releases from 26.1 on are unobfuscated and need none of this (docs/mc-1.21.11.md).
//
//   java --add-exports java.base/jdk.internal.org.objectweb.asm=ALL-UNNAMED \
//        --add-exports java.base/jdk.internal.org.objectweb.asm.commons=ALL-UNNAMED \
//        tools/remap/Remap.java IN.jar MAPPINGS.txt OUT.jar
//
// It uses the ASM the JDK carries inside java.base (JDK 21 to 25), so it needs no download. Names are
// mapped through the class hierarchy (a call on a subclass finds the method its superclass declares),
// lambdas included. Members the mapping does not list (those of libraries and of the JDK) keep their names.
import jdk.internal.org.objectweb.asm.ClassReader;
import jdk.internal.org.objectweb.asm.ClassVisitor;
import jdk.internal.org.objectweb.asm.ClassWriter;
import jdk.internal.org.objectweb.asm.Handle;
import jdk.internal.org.objectweb.asm.MethodVisitor;
import jdk.internal.org.objectweb.asm.Opcodes;
import jdk.internal.org.objectweb.asm.Type;
import jdk.internal.org.objectweb.asm.commons.ClassRemapper;
import jdk.internal.org.objectweb.asm.commons.Remapper;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Enumeration;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;

public class Remap {
    /** Obfuscated internal class name to Mojang's. */
    static final Map<String, String> classes = new HashMap<>();
    /** Mojang internal class name to the obfuscated one. */
    static final Map<String, String> toObf = new HashMap<>();
    /** `obfOwner.obfName obfDescriptor` to Mojang's method name. */
    static final Map<String, String> methods = new HashMap<>();
    /** `obfOwner.obfName` to Mojang's field name. */
    static final Map<String, String> fields = new HashMap<>();
    /** Obfuscated class to its superclass and interfaces (obfuscated names). */
    static final Map<String, List<String>> parents = new HashMap<>();
    static int conflicts = 0;

    static final Pattern CLASS = Pattern.compile("^(\\S+) -> (\\S+):$");
    static final Pattern METHOD =
            Pattern.compile("^\\s+(?:\\d+:\\d+:)?(\\S+) (\\S+)\\((.*)\\)(?::\\d+(?::\\d+)?)? -> (\\S+)$");
    static final Pattern FIELD = Pattern.compile("^\\s+(\\S+) (\\S+) -> (\\S+)$");

    public static void main(String[] args) throws IOException {
        if (args.length != 3) {
            System.err.println("usage: Remap IN.jar MAPPINGS.txt OUT.jar");
            System.exit(2);
        }
        readMappings(Path.of(args[1]));
        try (ZipFile in = new ZipFile(args[0])) {
            readHierarchy(in);
            write(in, Path.of(args[2]));
        }
        System.err.println("remapped " + classes.size() + " classes, " + methods.size() + " methods, "
                + fields.size() + " fields (" + conflicts + " method names listed twice, first kept)");
    }

    static String internal(String dotted) {
        return dotted.replace('.', '/');
    }

    /** A Java source type (`int`, `java.lang.String[]`) as a descriptor of obfuscated names. */
    static String descriptor(String type) {
        int dims = 0;
        while (type.endsWith("[]")) {
            type = type.substring(0, type.length() - 2);
            dims++;
        }
        String base = switch (type) {
            case "void" -> "V";
            case "boolean" -> "Z";
            case "byte" -> "B";
            case "char" -> "C";
            case "short" -> "S";
            case "int" -> "I";
            case "long" -> "J";
            case "float" -> "F";
            case "double" -> "D";
            default -> {
                String name = internal(type);
                yield "L" + toObf.getOrDefault(name, name) + ";";
            }
        };
        return "[".repeat(dims) + base;
    }

    static void readMappings(Path file) throws IOException {
        // Classes first: a member's descriptor names classes that are listed after it.
        try (BufferedReader r = Files.newBufferedReader(file)) {
            for (String line; (line = r.readLine()) != null; ) {
                if (line.startsWith("#") || line.startsWith(" ")) {
                    continue;
                }
                Matcher m = CLASS.matcher(line);
                if (m.matches()) {
                    String mojang = internal(m.group(1));
                    String obf = internal(m.group(2));
                    classes.put(obf, mojang);
                    toObf.put(mojang, obf);
                }
            }
        }
        try (BufferedReader r = Files.newBufferedReader(file)) {
            String owner = null;
            for (String line; (line = r.readLine()) != null; ) {
                if (line.startsWith("#") || line.isBlank()) {
                    continue;
                }
                Matcher c = CLASS.matcher(line);
                if (c.matches()) {
                    owner = internal(c.group(2));
                    continue;
                }
                if (owner == null) {
                    continue;
                }
                Matcher m = METHOD.matcher(line);
                if (m.matches()) {
                    String name = m.group(2);
                    if (name.contains(".")) {
                        continue; // an inlined frame of another class, not a member of this one
                    }
                    StringBuilder desc = new StringBuilder("(");
                    String params = m.group(3);
                    if (!params.isEmpty()) {
                        for (String p : params.split(",")) {
                            desc.append(descriptor(p.trim()));
                        }
                    }
                    desc.append(')').append(descriptor(m.group(1)));
                    String key = owner + "." + m.group(4) + desc;
                    String old = methods.putIfAbsent(key, name);
                    if (old != null && !old.equals(name)) {
                        conflicts++;
                    }
                    continue;
                }
                Matcher f = FIELD.matcher(line);
                if (f.matches()) {
                    fields.putIfAbsent(owner + "." + f.group(3), f.group(2));
                }
            }
        }
    }

    static void readHierarchy(ZipFile in) throws IOException {
        for (Enumeration<? extends ZipEntry> e = in.entries(); e.hasMoreElements(); ) {
            ZipEntry entry = e.nextElement();
            if (!entry.getName().endsWith(".class")) {
                continue;
            }
            try (InputStream s = in.getInputStream(entry)) {
                ClassReader reader = new ClassReader(s);
                List<String> up = new ArrayList<>();
                if (reader.getSuperName() != null) {
                    up.add(reader.getSuperName());
                }
                up.addAll(List.of(reader.getInterfaces()));
                parents.put(reader.getClassName(), up);
            }
        }
    }

    /** The key under which `name` is declared, looking at `owner` and then everything above it. */
    static String find(Map<String, String> table, String owner, String name, String desc) {
        Set<String> seen = new HashSet<>();
        ArrayDeque<String> queue = new ArrayDeque<>();
        queue.add(owner);
        while (!queue.isEmpty()) {
            String c = queue.poll();
            if (!seen.add(c)) {
                continue;
            }
            String hit = table.get(c + "." + name + (desc == null ? "" : desc));
            if (hit != null) {
                return hit;
            }
            queue.addAll(parents.getOrDefault(c, List.of()));
        }
        return null;
    }

    static final Remapper REMAPPER = new Remapper() {
        @Override
        public String map(String internalName) {
            return classes.getOrDefault(internalName, internalName);
        }

        @Override
        public String mapMethodName(String owner, String name, String descriptor) {
            if (owner == null || owner.startsWith("[")) {
                return name;
            }
            String hit = find(methods, owner, name, descriptor);
            return hit != null ? hit : name;
        }

        @Override
        public String mapFieldName(String owner, String name, String descriptor) {
            if (owner == null || owner.startsWith("[")) {
                return name;
            }
            String hit = find(fields, owner, name, null);
            return hit != null ? hit : name;
        }
    };

    /** Gives a lambda's invokedynamic the Mojang name of the interface method it implements. */
    static class Lambdas extends ClassVisitor {
        Lambdas(ClassVisitor next) {
            super(Opcodes.ASM9, next);
        }

        @Override
        public MethodVisitor visitMethod(int access, String name, String desc, String sig, String[] exc) {
            return new MethodVisitor(Opcodes.ASM9, super.visitMethod(access, name, desc, sig, exc)) {
                @Override
                public void visitInvokeDynamicInsn(String n, String d, Handle bsm, Object... args) {
                    if (bsm.getOwner().equals("java/lang/invoke/LambdaMetafactory")
                            && args.length > 0 && args[0] instanceof Type sam) {
                        String iface = Type.getReturnType(d).getInternalName();
                        String hit = find(methods, iface, n, sam.getDescriptor());
                        if (hit != null) {
                            n = hit;
                        }
                    }
                    super.visitInvokeDynamicInsn(n, d, bsm, args);
                }
            };
        }
    }

    static void write(ZipFile in, Path out) throws IOException {
        try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(out))) {
            for (Enumeration<? extends ZipEntry> e = in.entries(); e.hasMoreElements(); ) {
                ZipEntry entry = e.nextElement();
                if (entry.isDirectory()) {
                    continue;
                }
                String name = entry.getName();
                byte[] bytes;
                try (InputStream s = in.getInputStream(entry)) {
                    bytes = s.readAllBytes();
                }
                if (name.endsWith(".class")) {
                    ClassReader reader = new ClassReader(bytes);
                    ClassWriter writer = new ClassWriter(0);
                    reader.accept(new Lambdas(new ClassRemapper(writer, REMAPPER)), 0);
                    bytes = writer.toByteArray();
                    name = REMAPPER.map(name.substring(0, name.length() - ".class".length())) + ".class";
                } else if (name.startsWith("META-INF/") && (name.endsWith(".SF") || name.endsWith(".RSA")
                        || name.endsWith(".DSA"))) {
                    continue; // signatures of the original classes no longer hold
                }
                zip.putNextEntry(new ZipEntry(name));
                zip.write(bytes);
                zip.closeEntry();
            }
        }
    }
}
