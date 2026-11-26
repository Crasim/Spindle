#!/usr/bin/env bash
# Build Spindle for Android: the native library, then the APK, signed.
#
# Needs a JDK, Python 3, Android SDK (platform 34, build-tools 34/35, NDK 27),
# Eclipse ECJ 4.12 (SPINDLE_ECJ_JAR), and the Rust target:
#   rustup target add aarch64-linux-android
# Set ANDROID_HOME (and ANDROID_NDK_HOME if the NDK isn't in $ANDROID_HOME/ndk/27.2.12479018).
# The APK is signed with android/spindle-development.keystore, made on first build.
set -euo pipefail
cd "$(dirname "$0")"
: "${ANDROID_HOME:?Set ANDROID_HOME to your Android SDK}"
NDK="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/27.2.12479018}"
BT="$ANDROID_HOME/build-tools/34.0.0"
BUILDER_BT="$ANDROID_HOME/build-tools/35.0.0"
PLATFORM="$ANDROID_HOME/platforms/android-34/android.jar"
ECJ=$(realpath "${SPINDLE_ECJ_JAR:-../target/android-builder-deps/ecj-4.12.jar}")
echo "69dad18a1fcacd342a7d44c5abf74f50e7529975553a24c64bce0b29b86af497  $ECJ" | sha256sum -c -
D8="$BUILDER_BT/lib/d8.jar"
APKSIG="$BUILDER_BT/lib/apksigner.jar"
HOST=$(ls "$NDK/toolchains/llvm/prebuilt" | head -1)
TC="$NDK/toolchains/llvm/prebuilt/$HOST/bin"
export CC_aarch64_linux_android="$TC/aarch64-linux-android26-clang"
export CXX_aarch64_linux_android="$TC/aarch64-linux-android26-clang++"
export AR_aarch64_linux_android="$TC/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TC/aarch64-linux-android26-clang"

# Each APK gets the next build number: shown in Spindle (Settings, at the bottom) and as
# the app's version in Android's app info, so you can tell an update arrived.
BUILD=$(( $(tr -d '\r' 2>/dev/null < build-number || echo 9) + 1 ))
VERSION=$(tr -d '\r' < ../Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
# The app reports the desktop's version: keep android/Cargo.toml's the same.
sed -i "0,/^version = .*/s//version = \"$VERSION\"/" Cargo.toml
export SPINDLE_ANDROID_BUILD="$BUILD"
echo "Spindle $VERSION, Android build $BUILD"

cargo build --locked --release --target aarch64-linux-android

OUT=target/apk
rm -rf "$OUT" && mkdir -p "$OUT/compiled" "$OUT/classes" "$OUT/lib/arm64-v8a" "$OUT/assets/apk-builder"
"$BT/aapt2" compile --dir res -o "$OUT/compiled"
"$BUILDER_BT/aapt2" link -o "$OUT/template.apk" -I "$PLATFORM" --manifest builder/ProjectManifest.xml
unzip -p "$OUT/template.apk" AndroidManifest.xml > "$OUT/assets/apk-builder/manifest.bin"
python3 - "$OUT/assets/apk-builder/platform-classes.jar" "$PLATFORM" "$BUILDER_BT/core-lambda-stubs.jar" <<'PY'
import sys
from zipfile import ZIP_DEFLATED, ZipFile
with ZipFile(sys.argv[1], "w", ZIP_DEFLATED) as out:
    written = set()
    for path in sys.argv[2:]:
        with ZipFile(path) as library:
            for name in library.namelist():
                if name.endswith(".class") and name not in written:
                    out.writestr(name, library.read(name))
                    written.add(name)
PY
"$BT/aapt2" link -o "$OUT/base.apk" -I "$PLATFORM" -A "$OUT/assets" --manifest AndroidManifest.xml \
    --version-code "$BUILD" --version-name "$VERSION (build $BUILD)" --replace-version \
    "$OUT"/compiled/*.flat
cp target/aarch64-linux-android/release/libspindle.so "$OUT/lib/arm64-v8a/"
javac --release 8 -classpath "$PLATFORM:$ECJ:$D8:$APKSIG" -d "$OUT/classes" \
    java/org/spindleapp/spindle/*.java java/org/spindleapp/spindle/builder/*.java \
    builder/src/org/spindleapp/spindle/builder/JavaDex.java
jar cf "$OUT/classes.jar" -C "$OUT/classes" .
"$BUILDER_BT/d8" --release --min-api 26 --lib "$PLATFORM" --output "$OUT" \
    "$OUT/classes.jar" "$ECJ" "$D8" "$APKSIG"
# The C++ runtime the audio library needs.
cp "$NDK/toolchains/llvm/prebuilt/$HOST/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so" "$OUT/lib/arm64-v8a/"
# Every symbol the library needs must exist on Android 8 (API 26) or in what the APK ships;
# a missing one makes Android refuse to load the app, which shows as a crash at launch.
SYS="$NDK/toolchains/llvm/prebuilt/$HOST/sysroot/usr/lib/aarch64-linux-android"
NM="$TC/llvm-nm"
SO="$OUT/lib/arm64-v8a/libspindle.so"
"$NM" -D --undefined-only "$SO" | awk '$1=="U"{print $2}' | sed 's/@.*//' | sort -u > "$OUT/needed.txt"
for lib in $("$TC/llvm-readelf" -d "$SO" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p'); do
    if [ -f "$OUT/lib/arm64-v8a/$lib" ]; then "$NM" -D --defined-only "$OUT/lib/arm64-v8a/$lib"
    elif [ -f "$SYS/26/$lib" ]; then "$NM" -D --defined-only "$SYS/26/$lib"
    else echo "Missing library $lib" >&2; exit 1; fi
done | awk '{print $NF}' | sed 's/@.*//' | sort -u > "$OUT/have.txt"
if comm -23 "$OUT/needed.txt" "$OUT/have.txt" | grep .; then
    echo "These symbols aren't available on Android 8; the app wouldn't start." >&2
    exit 1
fi
(cd "$OUT" && zip -q -r base.apk lib classes*.dex)
# ECJ also needs its parser tables and diagnostics at runtime.
python3 - "$OUT/base.apk" "$ECJ" "$D8" "$APKSIG" <<'PY'
import sys
from zipfile import ZIP_DEFLATED, ZipFile
with ZipFile(sys.argv[1], "a", ZIP_DEFLATED) as apk:
    written = set(apk.namelist())
    for index, path in enumerate(sys.argv[2:]):
        with ZipFile(path) as jar:
            for name in jar.namelist():
                if name.endswith((".class", "/")):
                    continue
                target = name
                if name.startswith("META-INF/"):
                    if name.upper().endswith((".SF", ".RSA", ".DSA", "MANIFEST.MF")):
                        continue
                    if not name.startswith("META-INF/services/"):
                        target = f"assets/apk-builder/notices/{index}/{name[9:]}"
                if target not in written:
                    apk.writestr(target, jar.read(name))
                    written.add(target)
PY
"$BT/zipalign" -f -p 4 "$OUT/base.apk" "$OUT/aligned.apk"
if [ ! -f spindle-development.keystore ]; then
    keytool -genkeypair -keystore spindle-development.keystore -storepass spindle -keypass spindle \
        -alias spindle -keyalg RSA -keysize 2048 -validity 10000 \
        -dname "CN=Spindle Development" >/dev/null
fi
"$BT/apksigner" sign --ks spindle-development.keystore --ks-pass pass:spindle --key-pass pass:spindle \
    --out "$OUT/Spindle.apk" "$OUT/aligned.apk"
"$BT/apksigner" verify "$OUT/Spindle.apk"
mv "$OUT/Spindle.apk" Spindle.apk
echo "$BUILD" > build-number
echo "Built $(pwd)/Spindle.apk"
