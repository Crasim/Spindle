# Build and install Spindle

[Spindle](../README.md)

## Build from source

Use Rust 1.89 or later. On Windows, install the MSVC C++ build tools. On Linux, install the development packages for ALSA, OpenGL, X11, and xkbcommon. On Ubuntu, the release build uses `pkg-config libasound2-dev libgl1-mesa-dev libx11-dev libxkbcommon-dev`. For a local generation test, start Ollama and pull a model.

```powershell
cargo test --locked
cargo run --release --locked
cargo run --release --locked -- --cli
cargo run --release --locked -- --models
cargo run --release --locked -- --ask qwen3.5:9b "Reply with two words."
```

While Spindle is running, rebuild into a separate folder with `cargo build --release --locked --offline --target-dir target/staged`. On Windows, **Start Spindle.vbs** picks the newest build and opens Spindle without a console. **Start Spindle.cmd** shows startup diagnostics.

On Windows and Linux, open **Settings › Preferences › Updates** and choose **Check for updates**, then **Download update**. When the download is ready, choose **Restart to update**. Spindle saves the workspace, verifies the download’s SHA-256 digest again, and opens the updated app in the same workspace. The desktop release workflow publishes assets for a Git tag that matches the version in `Cargo.toml`. Builds from a GitHub checkout use that checkout's origin for downloads. Builds from source archives can set `SPINDLE_RELEASE_REPO` to the GitHub owner and repository.

The current working directory identifies the workspace, and each workspace keeps its own chats. Start Spindle from the same folder each time (for example with **Start Spindle.vbs**). Starting the executable from its build folder opens a different, empty workspace. Projects open inside the one window from the sidebar. Desktop and interactive CLI share one session lock per workspace. Chats, pending work, attachment snapshots, and installed skills are saved in a Windows-user-encrypted session under `%LOCALAPPDATA%\Spindle\state`. Existing installs reopen their encrypted storage in place. One-shot queries use no session file.

## Android

`android/Spindle.apk` is Spindle for Android phones (64-bit ARM, Android 8 or newer). It is the same program as the desktop app, laid out in one column: the sidebar opens from the menu button at the top left, and side panels cover the conversation until you close them.

- **Install:** copy the APK to the phone and open it (allow installing from that app when Android asks), or `adb install -r android/Spindle.apk`.
- **Set up:** Settings › Sync › Join, then pair with a desktop (Add a device there, type its code on the phone, type the phone's number on the desktop). On the same Wi-Fi the phone syncs with your desktops directly.
- **Models:** add your model servers by address (for example `http://model-host.local:11434` for Ollama); `localhost` on the phone means the phone itself.
- **Microphone:** grant it in Android Settings › Apps › Spindle › Permissions; the app can't ask yet.
- **What's different:** Android offers isolated Java APK builds. The desktop Python and C/C++ sandbox, built-in browser, and pasting images are desktop features. Spindle syncs while the Android app is open.
- **Build:** Run `android/build-apk.sh` in Linux or WSL with a JDK, Python 3, Android SDK 34, Build Tools 34 and 35, NDK 27, and `rustup target add aarch64-linux-android`. Set `SPINDLE_ECJ_JAR` to the ECJ 4.12 jar used by the [build probe](../android/builder/README.md). Keep `android/spindle-development.keystore` for updates to this package. The renamed Android app installs separately from earlier builds. Sync shared data from the earlier app before removing it.
