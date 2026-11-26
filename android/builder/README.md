# Built-in Java builder development probe

The target is an offline Spindle build tool for small Java projects. The phone should need only Spindle. Termux's ECJ packaging recipe confirms that Eclipse's compiler can be bundled as DEX with its parser resources. This prototype calls ECJ and D8 with Java APIs and a fixed class library. The worker takes Java text and returns DEX bytes in memory. The compilation profile disables annotation processing and fixes the source level, classpath, and output format.

The probe APK runs a fixed two-source Android Activity fixture in a non-exported Android isolated service. It first checks for a separate UID and attempts to read a private preparation marker created by the caller. It then loads its packaged framework classes and compiles the fixture. The caller times out after 45 seconds. The main app accepts the fixture's bounded DEX result, combines it with a bundled manifest, and uses apksig and an Android Keystore debug key to sign and verify the result. The device screen reports each stage and enables **Save fixture APK** after verification.

## Build and verify

Use the workspace Android API 34 SDK, Build Tools 35.0.0, a host JDK, Python 3, and the Eclipse ECJ 4.12 jar. The script checks the ECJ checksum before use.

```sh
export SPINDLE_ECJ_JAR=/path/to/ecj-4.12.jar
bash android/builder/verify.sh --probe
```

The script tests successful compilation and a syntax error on the host. It links and signs the resulting Java fixture with the host SDK, then writes `android/target/java-builder/JavaFixture.apk`. With `--probe` it also builds `SpindleBuildProbe.apk` in that folder for testing Android's isolated service and ART. Open that APK on a test phone and launch **Spindle Build Probe**. The development probe has its own package ID and signing key.

On November 24 the diagnostic passed on an Armor 28 Ultra. The worker reported 1,360 bytes of DEX and denied access to the main app's private marker. The phone signed and verified the APK, the Android document picker saved it to Downloads, and the exported APK installed and launched as **Spindle Java Fixture**.

## Android Spindle integration

Android Spindle's `build_apk` tool takes explicitly named Java source files, a package name, a launcher Activity, and a label. Approval sends a bounded snapshot of those sources to a non-exported isolated service. The service checks its UID and permission state, compiles through ECJ with annotation processing disabled, and returns a single bounded DEX. The app packages it with a fixed manifest and signs and verifies the APK with a per-package Android Keystore key. The resulting chat file has a Download action.

The Java profile supports programmatic Android UI. Resource compilation, Kotlin, Gradle, compiler plugins, native libraries, and running generated apps require separate profiles. The build waits at most 45 seconds for the isolated service to compile.

The [Termux ECJ recipe](https://github.com/termux/termux-packages/blob/master/packages/ecj/build.sh), [Termux Android build tools](https://github.com/termux/android-build-tools), and [Android isolated-service documentation](https://developer.android.com/guide/topics/manifest/service-element) inform this approach. The prototype downloads or installs nothing on the phone.
