package org.spindleapp.spindle.builder;

import com.android.tools.r8.ByteDataView;
import com.android.tools.r8.D8;
import com.android.tools.r8.D8Command;
import com.android.tools.r8.DexIndexedConsumer;
import com.android.tools.r8.DiagnosticsHandler;
import com.android.tools.r8.origin.Origin;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;
import org.eclipse.jdt.core.compiler.CategorizedProblem;
import org.eclipse.jdt.internal.compiler.ClassFile;
import org.eclipse.jdt.internal.compiler.Compiler;
import org.eclipse.jdt.internal.compiler.DefaultErrorHandlingPolicies;
import org.eclipse.jdt.internal.compiler.batch.CompilationUnit;
import org.eclipse.jdt.internal.compiler.classfmt.ClassFileReader;
import org.eclipse.jdt.internal.compiler.classfmt.ClassFormatException;
import org.eclipse.jdt.internal.compiler.env.INameEnvironment;
import org.eclipse.jdt.internal.compiler.env.NameEnvironmentAnswer;
import org.eclipse.jdt.internal.compiler.impl.CompilerOptions;
import org.eclipse.jdt.internal.compiler.problem.DefaultProblemFactory;

/** Fixed Java 8 compilation profile for an Android worker, without shell commands or processors. */
final class JavaDex {
    private static final int MAX_SOURCE_SIZE = 128 * 1024;
    private static final int MAX_CLASS_SIZE = 1024 * 1024;
    private static final int MAX_PLATFORM_SIZE = 64 * 1024 * 1024;

    static byte[] compile(Map<String, String> sources, InputStream platformJar) throws Exception {
        return compileActivity(sources, platformJar, null);
    }

    static byte[] compileActivity(Map<String, String> sources, InputStream platformJar, String activity)
            throws Exception {
        if (sources.isEmpty() || sources.size() > 64) {
            throw new IllegalArgumentException("Provide between 1 and 64 Java source files.");
        }
        int characters = 0;
        for (Map.Entry<String, String> source : sources.entrySet()) {
            String name = source.getKey();
            String contents = source.getValue();
            if (name.length() > 240 || !name.matches("([a-zA-Z_$][a-zA-Z0-9_$]*/)*[a-zA-Z_$][a-zA-Z0-9_$]*[.]java")) {
                throw new IllegalArgumentException("Use a Java file name relative to the source root.");
            }
            if (contents.length() > MAX_SOURCE_SIZE - characters) {
                throw new IllegalArgumentException("Java sources exceed the total length limit.");
            }
            characters += contents.length();
        }
        Platform platform = new Platform(platformJar);
        Map<String, byte[]> classes = compileClasses(sources, platform);
        if (activity != null && !classes.containsKey(activity.replace('.', '/'))) {
            throw new IllegalArgumentException("No compiled class matches launcher Activity " + activity);
        }
        DexOutput output = new DexOutput();
        D8Command.Builder dex = D8Command.builder().setMinApiLevel(26)
                .setProgramConsumer(output);
        for (byte[] data : classes.values()) {
            dex.addClassProgramData(data, Origin.unknown());
        }
        ExecutorService executor = Executors.newSingleThreadExecutor();
        try {
            D8.run(dex.build(), executor);
        } finally {
            executor.shutdownNow();
        }
        if (output.files.size() != 1 || !output.files.containsKey(0)) {
            throw new IOException("This build profile requires a single DEX output.");
        }
        return output.files.get(0);
    }

    private static final class DexOutput implements DexIndexedConsumer {
        private final Map<Integer, byte[]> files = new HashMap<>();

        @Override
        public synchronized void accept(int index, ByteDataView data,
                                        Set<String> descriptors, DiagnosticsHandler handler) {
            files.put(index, data.copyByteData());
        }

        @Override
        public void finished(DiagnosticsHandler handler) {}
    }

    private static Map<String, byte[]> compileClasses(Map<String, String> sources, Platform platform) {
        CompilationUnit[] units = sources.entrySet().stream()
                .map(source -> new CompilationUnit(source.getValue().toCharArray(), source.getKey(), "UTF-8"))
                .toArray(CompilationUnit[]::new);
        Map<String, String> settings = new HashMap<>();
        settings.put(CompilerOptions.OPTION_Source, CompilerOptions.VERSION_1_8);
        settings.put(CompilerOptions.OPTION_Compliance, CompilerOptions.VERSION_1_8);
        settings.put(CompilerOptions.OPTION_TargetPlatform, CompilerOptions.VERSION_1_8);
        settings.put(CompilerOptions.OPTION_Process_Annotations, CompilerOptions.DISABLED);
        settings.put(CompilerOptions.OPTION_MaxProblemPerUnit, "10");
        Map<String, byte[]> classes = new LinkedHashMap<>();
        StringBuilder errors = new StringBuilder();
        Compiler compiler = new Compiler(platform, DefaultErrorHandlingPolicies.exitAfterAllProblems(),
                new CompilerOptions(settings), result -> {
                    if (result.hasErrors()) {
                        for (CategorizedProblem problem : result.getErrors()) {
                            if (errors.length() < 4096) {
                                errors.append(problem.getOriginatingFileName()).append(':')
                                        .append(problem.getSourceLineNumber()).append(' ')
                                        .append(problem.getMessage()).append('\n');
                            }
                        }
                        return;
                    }
                    for (ClassFile file : result.getClassFiles()) {
                        classes.put(new String(file.fileName()), file.getBytes());
                    }
                }, new DefaultProblemFactory(Locale.ROOT));
        compiler.useSingleThread = true;
        compiler.compile(units);
        if (errors.length() > 0 || classes.isEmpty()) {
            throw new IllegalArgumentException(errors.length() > 0 ? errors.toString() : "No Java classes were generated.");
        }
        return classes;
    }

    private static String path(char[][] packageName) {
        StringBuilder path = new StringBuilder();
        if (packageName != null) {
            for (char[] part : packageName) {
                if (path.length() > 0) path.append('/');
                path.append(part);
            }
        }
        return path.toString();
    }

    private static String child(String parent, char[] name) {
        return parent.isEmpty() ? new String(name) : parent + "/" + new String(name);
    }

    /** The worker looks up platform types in a read-only APK asset. */
    private static final class Platform implements INameEnvironment {
        private final Map<String, byte[]> classes = new HashMap<>();
        private final Set<String> packages = new HashSet<>();

        Platform(InputStream input) throws IOException {
            try (ZipInputStream jar = new ZipInputStream(input)) {
                int total = 0;
                byte[] buffer = new byte[8192];
                for (ZipEntry entry; (entry = jar.getNextEntry()) != null; ) {
                    String name = entry.getName();
                    if (!name.endsWith(".class")) continue;
                    ByteArrayOutputStream data = new ByteArrayOutputStream();
                    for (int count; (count = jar.read(buffer)) != -1; ) {
                        total += count;
                        if (total > MAX_PLATFORM_SIZE || data.size() + count > MAX_CLASS_SIZE) {
                            throw new IOException("Platform class library exceeds the build limits.");
                        }
                        data.write(buffer, 0, count);
                    }
                    classes.put(name, data.toByteArray());
                    for (int slash = name.lastIndexOf('/'); slash > 0; slash = name.lastIndexOf('/', slash - 1)) {
                        packages.add(name.substring(0, slash));
                    }
                }
            }
            if (!classes.containsKey("java/lang/Object.class")) {
                throw new IOException("The platform class library is missing Java core types.");
            }
        }

        private NameEnvironmentAnswer type(String name) {
            String entry = name + ".class";
            byte[] data = classes.get(entry);
            if (data == null) return null;
            try {
                return new NameEnvironmentAnswer(new ClassFileReader(data, entry.toCharArray()), null);
            } catch (ClassFormatException invalid) {
                throw new IllegalStateException("Invalid platform class " + entry, invalid);
            }
        }

        @Override
        public NameEnvironmentAnswer findType(char[][] className) {
            return type(path(className));
        }

        @Override
        public NameEnvironmentAnswer findType(char[] className, char[][] packageName) {
            return type(child(path(packageName), className));
        }

        @Override
        public boolean isPackage(char[][] parentPackage, char[] name) {
            String fullName = child(path(parentPackage), name);
            return packages.contains(fullName) || !classes.containsKey(fullName + ".class");
        }

        @Override
        public void cleanup() {}
    }
}
