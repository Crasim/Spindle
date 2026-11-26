---
name: android-apk
description: Choose, run, or troubleshoot a build and verification workflow for an Android APK.
---

# Build an Android APK

Inspect the project's manifest, source languages, dependencies, and existing build files. Identify the tools this session can actually call and where they run. Choose a route that fits the project, the requested build location, and the user's approvals. Keep working on source or build configuration when only the final compile is blocked.

## Available routes

- **Spindle's on-device builder.** When the chat lists `build_apk`, create or read the app's Java source files and call it with the application package, fully qualified launcher Activity, display label, and the explicit relative Java source paths. For a new app suited to this profile, use Java 8 and Android framework APIs with a programmatic UI. The builder generates the manifest, compiles in Android's isolated service and signs on the device after approval. It accepts up to 64 Java sources totaling 128 KiB. This profile covers Java apps with a launcher Activity. Kotlin, Gradle scripts, resource XML, test runners, libraries, and native compilation require a compatible build tool. The APK appears in the chat with a Download action.
- **Termux on the phone.** For a dependency-free Java/XML app with `AndroidManifest.xml`, `src/`, and optional `res/`, use the project's existing script or `tools/build-termux-apk.py` when present. The helper needs Python, `javac`, D8, `aapt2`, `zipalign`, `apksigner`, `keytool`, and a local Android platform jar. It expects Java code that works without generated `R.java`. In an authorized Termux environment, confirm tools with `command -v`. Termux runs in its own app. If the session has file tools only, prepare the source and give the exact handoff command.

  ```sh
  python3 /path/to/build-termux-apk.py /path/to/project \
    --android-jar /path/to/android.jar --min-api 24 \
    --output ~/storage/downloads/Example.apk
  ```

  Match the minimum API to the manifest. Shared Downloads requires Termux storage access. Open the signed APK from Downloads to install it when installation is requested.
- **Existing Gradle or other project tooling.** Use the checked-in wrapper, project-documented variant, and compatible JDK and SDK on an available, approved host. A typical Gradle debug build uses `./gradlew assembleDebug` or `gradlew.bat assembleDebug` from the project root and writes under `<module>/build/outputs/apk/`. This route supports projects that already use Kotlin, resource generation, or Android libraries. Preserve the existing build system unless the user asks for a port. [Android command-line build guide](https://developer.android.com/build/building-cmdline)

## Verify the result

Check the actual artifact path and signature with `apksigner verify --verbose` or the build tool's verification result. For an update, preserve the application ID and signer and advance the version code. Report the APK only after the build confirms it exists. Report build, signature verification, installation, and launch as separate outcomes. Install or launch a generated app only when the user authorizes that step.
