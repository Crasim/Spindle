Spindle relies on the work of these projects and their contributors.

### Interface and embedded fonts

- **egui and eframe** provide the interface. The Android build includes a modified version of **egui_glow** with higher-precision shaders. Their upstream crates are available under MIT or Apache-2.0. [egui](https://github.com/emilk/egui) · [egui_glow 0.31.1](https://crates.io/crates/egui_glow/0.31.1)
- **Hack** from Source Foundry includes MIT-licensed Hack work, public-domain DejaVu work, and Bitstream Vera under the Bitstream Vera License. [Hack](https://github.com/source-foundry/Hack)
- **Noto Emoji** from Google uses the SIL Open Font License 1.1 for the bundled font. [Noto Emoji](https://github.com/googlefonts/noto-emoji)
- **Ubuntu Light** from Canonical and Dalton Maag uses the Ubuntu Font Licence 1.0. [Ubuntu Font Family](https://design.ubuntu.com/font)
- **Emoji Icon Font** from John Slegers uses the MIT License. [Emoji Icon Font](https://github.com/jslegers/emoji-icon-font)
- **Wry** from Tauri provides the Windows web view integration under MIT or Apache-2.0. [Wry](https://github.com/tauri-apps/wry)

### Bundled data and Android tooling

- **Natural Earth** provides the public-domain country boundaries used to turn an approximate device location into a country code. [Natural Earth terms](https://www.naturalearthdata.com/about/terms-of-use/)
- **Eclipse JDT ECJ** compiles Java on Android under the Eclipse Public License 2.0. [Eclipse JDT Core](https://github.com/eclipse-jdt/eclipse.jdt.core) · [Eclipse Public License](https://www.eclipse.org/legal/epl-2.0/)
- **D8 from the R8 project** converts class files to Android DEX under BSD-3-Clause. **apksig** signs built APKs under Apache-2.0. The builder also includes Android SDK platform stubs. [R8 license](https://r8.googlesource.com/r8/+/refs/heads/main/LICENSE) · [AOSP apksig](https://android.googlesource.com/platform/tools/apksig/) · [Android SDK](https://developer.android.com/tools/releases/platforms)
- **Oboe** provides Android audio under Apache-2.0. **LLVM libc++** supplies the Android C++ runtime under Apache-2.0 with the LLVM exception and includes additional upstream notices. [Oboe](https://github.com/google/oboe) · [LLVM libc++ license](https://github.com/llvm/llvm-project/blob/main/libcxx/LICENSE.TXT)
- Desktop Python setup can download an **Astral Python Build Standalone** distribution of **CPython**. The build tooling uses MPL-2.0. The distribution carries CPython's license and notices for its included components. [Python Build Standalone](https://github.com/astral-sh/python-build-standalone/blob/main/LICENSE) · [CPython license](https://docs.python.org/3/license.html)

### Design references

The project design draws workflow ideas from **OpenCode** and **Aider**. Spindle's Android builder design references build recipes and tools from **Termux**. [OpenCode](https://github.com/anomalyco/opencode) · [Aider](https://github.com/Aider-AI/aider) · [Termux](https://github.com/termux/termux-packages)
