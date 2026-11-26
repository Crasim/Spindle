package org.spindleapp.spindle.builder;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;

/** Rewrite three strings in Spindle's own aapt2-linked manifest, preserving their indexes. */
final class ApkManifest {
    static byte[] render(byte[] template, String packageName, String activity, String label)
            throws IOException {
        ByteBuffer xml = littleEndian(template);
        int poolOffset = 8;
        int headerSize = 28;
        int poolSize = xml.getInt(poolOffset + 4);
        int count = xml.getInt(poolOffset + 8);
        int flags = xml.getInt(poolOffset + 16);
        int dataOffset = xml.getInt(poolOffset + 20);
        if (xml.getInt(0) != 0x00080003 || xml.getInt(poolOffset) != 0x001c0001 ||
                xml.getInt(poolOffset + 12) != 0 || (flags & 0x100) != 0 ||
                count < 3 || count > 100 || dataOffset != headerSize + 4 * count ||
                poolSize < dataOffset || poolSize > template.length - poolOffset) {
            throw new IOException("Unsupported APK manifest template.");
        }
        int[] offsets = new int[count];
        int changed = 0;
        ByteArrayOutputStream strings = new ByteArrayOutputStream();
        for (int i = 0; i < count; i++) {
            int pos = poolOffset + dataOffset + xml.getInt(poolOffset + headerSize + 4 * i);
            int chars = xml.getShort(pos) & 0xffff;
            String value = new String(template, pos + 2, 2 * chars, StandardCharsets.UTF_16LE);
            if (value.equals("com.example.spindleplaceholder")) {
                changed |= 1;
                value = packageName;
            } else if (value.equals("com.example.spindleplaceholder.LauncherActivity")) {
                changed |= 2;
                value = activity;
            } else if (value.equals("Spindle Placeholder Label")) {
                changed |= 4;
                value = label;
            }
            offsets[i] = strings.size();
            strings.write(value.length() & 0xff);
            strings.write(value.length() >> 8);
            byte[] bytes = value.getBytes(StandardCharsets.UTF_16LE);
            strings.write(bytes, 0, bytes.length);
            strings.write(0);
            strings.write(0);
        }
        if (changed != 7) throw new IOException("Missing APK manifest template values.");
        while (strings.size() % 4 != 0) strings.write(0);
        int newPoolSize = dataOffset + strings.size();
        int newSize = template.length + newPoolSize - poolSize;
        ByteBuffer out = littleEndian(new byte[newSize]);
        out.putInt(xml.getInt(0)).putInt(newSize);
        out.putInt(xml.getInt(poolOffset)).putInt(newPoolSize).putInt(count);
        out.putInt(0).putInt(flags & ~1).putInt(dataOffset).putInt(0);
        for (int offset : offsets) out.putInt(offset);
        out.put(strings.toByteArray());
        int afterPool = poolOffset + poolSize;
        out.put(template, afterPool, template.length - afterPool);
        return out.array();
    }

    private static ByteBuffer littleEndian(byte[] bytes) {
        return ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN);
    }
}
