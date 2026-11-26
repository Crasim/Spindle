#!/usr/bin/env bash
# Development fixture for the Android Java builder. Requires the release SDK and JDK on the host.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
: "${ANDROID_HOME:?Set ANDROID_HOME to the workspace Android SDK}"
: "${SPINDLE_ECJ_JAR:?Set SPINDLE_ECJ_JAR to the Eclipse ECJ 4.12 jar}"
ecj="$(realpath "$SPINDLE_ECJ_JAR")"
echo "69dad18a1fcacd342a7d44c5abf74f50e7529975553a24c64bce0b29b86af497  $ecj" | sha256sum -c -
platform="$ANDROID_HOME/platforms/android-34/android.jar"
tools="$ANDROID_HOME/build-tools/35.0.0"
d8="$tools/lib/d8.jar"
apksigner="$tools/lib/apksigner.jar"
output="$here/../target/java-builder"
mkdir -p "$output"
work="$(mktemp -d "$output/work.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT
mkdir -p "$work/classes" "$work/dex" "$work/assets"
sources=("$here"/src/org/spindleapp/spindle/builder/*.java)

javac -g --release 8 -classpath "$platform:$ecj:$d8:$apksigner" -d "$work/classes" "${sources[@]}"
cat > "$work/Verify.java" <<'EOF'
package org.spindleapp.spindle.builder;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.Collections;

public class Verify {
    public static void main(String[] args) throws Exception {
        try (InputStream platform = Files.newInputStream(Paths.get(args[0]))) {
            Files.write(Paths.get(args[1]), BuildFixture.run(platform));
        }
        try (InputStream platform = Files.newInputStream(Paths.get(args[0]))) {
            try {
                JavaDex.compile(Collections.singletonMap("Broken.java", "class Broken { @ }"), platform);
                throw new AssertionError("Invalid Java compiled.");
            } catch (IllegalArgumentException expected) {
                if (!expected.getMessage().contains("Broken.java:1")) throw expected;
            }
        }
        System.out.println("Java-to-DEX fixture and syntax diagnostics passed.");
    }
}
EOF
javac --release 8 -classpath "$work/classes" -d "$work" "$work/Verify.java"
java -classpath "$work:$work/classes:$ecj:$d8" org.spindleapp.spindle.builder.Verify \
    "$platform" "$work/classes.dex"

cat > "$work/FixtureManifest.xml" <<'EOF'
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="org.spindleapp.localfixture" android:versionCode="1">
    <uses-sdk android:minSdkVersion="26" android:targetSdkVersion="34"/>
    <application android:label="Spindle Java Fixture">
        <activity android:name=".MainActivity" android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.MAIN"/>
                <category android:name="android.intent.category.LAUNCHER"/>
            </intent-filter>
        </activity>
    </application>
</manifest>
EOF

key="$output/fixture-debug.p12"
if [[ ! -f "$key" ]]; then
    keytool -genkeypair -keystore "$key" -storepass android -keypass android -alias fixture \
        -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Spindle Builder Test"
fi
sign() {
    local apk="$1"
    local name="$2"
    "$tools/zipalign" -f -p 4 "$apk" "$work/aligned.apk"
    "$tools/apksigner" sign --ks "$key" --ks-key-alias fixture --ks-pass pass:android \
        --out "$work/signed.apk" "$work/aligned.apk"
    "$tools/apksigner" verify "$work/signed.apk"
    unzip -tq "$work/signed.apk"
    mv "$work/signed.apk" "$output/$name"
}

"$tools/aapt2" link -o "$work/fixture.apk" -I "$platform" --manifest "$work/FixtureManifest.xml"
(cd "$work" && zip -q fixture.apk classes.dex)
sign "$work/fixture.apk" JavaFixture.apk
"$tools/aapt2" dump badging "$output/JavaFixture.apk" | awk '/^(package:|sdkVersion:|launchable-activity:)/'

if [[ "${1:-}" != "--probe" ]]; then
    exit 0
fi

# The development APK includes the locally installed tool jars. It is separate from Spindle.
jar cf "$work/probe.jar" -C "$work/classes" .
"$tools/d8" --release --min-api 26 --lib "$platform" --output "$work/dex" \
    "$work/probe.jar" "$ecj" "$d8" "$apksigner"
unzip -p "$work/fixture.apk" AndroidManifest.xml > "$work/assets/fixture-manifest.bin"
python3 - "$platform" "$work/assets/platform-classes.jar" <<'PY'
import sys
from zipfile import ZipFile, ZIP_DEFLATED
with ZipFile(sys.argv[1]) as platform, ZipFile(sys.argv[2], "w", ZIP_DEFLATED) as classes:
    for name in platform.namelist():
        if name.endswith(".class"):
            classes.writestr(name, platform.read(name))
PY
"$tools/aapt2" link -o "$work/probe.apk" -I "$platform" -A "$work/assets" --manifest "$here/AndroidManifest.xml"
python3 - "$work/probe.apk" "$work/dex" "$ecj" "$d8" "$apksigner" <<'PY'
import sys
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
with ZipFile(sys.argv[1], "a", ZIP_DEFLATED) as apk:
    for dex in sorted(Path(sys.argv[2]).glob("classes*.dex")):
        apk.write(dex, dex.name)
    for index, tool in enumerate(sys.argv[3:]):
        with ZipFile(tool) as jar:
            for name in jar.namelist():
                if name.endswith((".class", "/")):
                    continue
                if name.startswith("META-INF/"):
                    if name.upper().endswith((".SF", ".RSA", ".DSA", "MANIFEST.MF")):
                        continue
                    if not name.startswith("META-INF/services/"):
                        apk.writestr("assets/tool-notices/" + str(index) + "/" + name[9:], jar.read(name))
                        continue
                apk.writestr(name, jar.read(name))
PY
sign "$work/probe.apk" SpindleBuildProbe.apk
echo "Built $output/SpindleBuildProbe.apk"
