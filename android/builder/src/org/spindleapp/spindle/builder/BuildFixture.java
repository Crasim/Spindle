package org.spindleapp.spindle.builder;

import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.Map;

final class BuildFixture {
    private static final String DESCRIPTOR = "Lorg/spindleapp/localfixture/MainActivity;";

    static byte[] run(InputStream platform) throws Exception {
        Map<String, String> sources = new LinkedHashMap<>();
        sources.put("org/spindleapp/localfixture/MainActivity.java",
                "package org.spindleapp.localfixture;\n"
                + "public final class MainActivity extends android.app.Activity {\n"
                + "  @Override public void onCreate(android.os.Bundle saved) {\n"
                + "    super.onCreate(saved);\n"
                + "    android.widget.TextView label = new android.widget.TextView(this);\n"
                + "    label.setText(Caption.text());\n"
                + "    setContentView(label);\n"
                + "  }\n"
                + "}\n");
        sources.put("org/spindleapp/localfixture/Caption.java",
                "package org.spindleapp.localfixture;\n"
                + "final class Caption {\n"
                + "  static String text() { return \"Built with Spindle's Java worker.\"; }\n"
                + "}\n");
        byte[] dex = JavaDex.compile(sources, platform);
        if (dex.length < 112 || !Arrays.equals(Arrays.copyOf(dex, 4), new byte[] {'d', 'e', 'x', '\n'})
                || !new String(dex, StandardCharsets.ISO_8859_1).contains(DESCRIPTOR)) {
            throw new IllegalStateException("The compiler did not produce the expected Activity in DEX.");
        }
        return dex;
    }

    private BuildFixture() {}
}
