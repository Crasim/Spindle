# Computer use

[Spindle](../README.md)

## Side panel

The Panel button opens **Viewer** for documents and previews, **Files** for chat attachments and outputs, **Library** for workspace files, and **Browser** for web pages beside the chat.

### Web browsing in Cowork

Ask for **Spindle’s built-in Browser panel** to open a public page beside the conversation on Windows. On Linux, the panel shows the page snapshot the model read. Browser Link uses the Edge or Chrome profile you already use, with its open tabs and sign-ins.

When a chat needs a web region, Spindle asks to use device location. Windows uses Location Services, Linux uses the desktop location portal, and Android requests approximate foreground location. Coordinates are mapped to a country locally using bundled public-domain boundaries. Only the country code is shared with the selected model provider for regional web tasks. If location is denied or unavailable, choose a country or continue without one. **Settings › Preferences › Web region** lets you change location permission or keep a manual override for travel or VPN use. [Natural Earth country boundaries](https://www.naturalearthdata.com/downloads/50m-cultural-vectors/)

The Browser panel opens as soon as Spindle starts opening a public page. On Windows, when the isolated read is refused, **Open and share here** opens the page in the Browser panel and shares its visible text and links with the selected model. Spindle continues the request in the same chat. **Use regular browser** in the conversation opens the alternative Browser Link flow with website and model approvals. In the connected browser, a pointer and highlighted border show when Spindle reads or interacts with the tab. Press **Esc** in that tab to revoke access.

Open **Settings › Computer use** and choose **Manage** on a browser card to change website access, review saved model permissions, or disconnect its active tab. The panel shows the installed Browser Link version and the version available with Spindle. **Update Browser Link** prepares a matching folder when the connected extension is older. Spindle verifies and unpacks the extension and opens the install guide. Use your browser’s **Manage extensions** page to load the prepared folder. A bundled copy is available offline. [Extension guide](../browser-extension/README.md)

For a named page, Spindle reuses a matching tab or opens the requested address after website approval. For a request about the current tab, it uses the active webpage. If there is no webpage and the request does not name one, enter its address directly in chat. The extension sends requests and results through Native Messaging. Its local host talks to Spindle over a private account-only connection. Both Chat and Cowork can request a read. Use Cowork for clicks and text entry.

Browser Link defaults to an empty **Allowlist**. When a model requests a page, the chat shows its website and offers **Allow this site**, **Allow all**, and **Deny**. Either Allow choice also approves the displayed model’s pending read once. **Allow this site** adds the exact origin. **Allow all** makes every site eligible for future model requests. Subsequent actions use your saved model permissions.

You can also add an exact origin or URL path in Computer use. The extension mirrors the URL rules and can change the mode, remove rules, or block the current domain. The available modes are **Allow all**, **Allowlist**, **Blocklist**, and **Block all**. An origin rule applies to that protocol, host, and port. Subdomains need separate entries. If Settings sync is enabled, the two URL lists sync across devices. Each device chooses its mode.

Spindle asks you to approve the selected model's reads, clicks, and text entry on the paired tab. Choose **Allow once**, **Allow for this conversation**, **Allow for this project**, or **Allow always**. Standing approvals apply to the exact model, URL rule, and action type and can be revoked in Computer use. **Allow always** applies across chats in the current workspace. Pairing and website rules still apply. Reads return visible page text and same-site link references with query strings and fragments removed. A link outside the allowed path can be cited. Opening it still requires an allowed URL and an approved action. Returned page content goes to the selected model provider. Password fields and file inputs are excluded. The separate disposable browser handles console and network diagnostics.

### Plugins and connectors

Cowork shows the model descriptions of installed skills and lets it load a relevant skill with `read_skill`. Your existing tool permissions apply to actions the skill suggests.

**Settings › Plugins** lists external tools installed on the device and opens a Cowork request to discover more. **Settings › Connectors** connects OneDrive, GitHub, Gmail, Google Drive, and Dropbox using a read-scoped access token. Tokens are verified against the service and encrypted on this device. Cowork can request read-only catalog lookups and Gmail snippets. Each lookup needs approval before results go to the selected model. Use **Disconnect** to remove the saved token.

HTML opens in **Preview** by default from a reply, **Files**, or **Library**. Use **Source** to inspect or copy the HTML. Windows and Android render inside the side panel. Linux opens a dedicated Chromium app window and uses the executable from **Settings → Sandbox → Model browser**. Inline scripts, same-folder relative pages and assets, and keyboard or touch controls work in the preview. The preview's network is restricted to its own local files, so bundle required scripts, styles, and images beside the HTML or inline. A synced preview can show the saved HTML even when its companion files are absent.

In Cowork, `preview_html` publishes a saved page as a clickable artifact and supports relative Markdown links to it. On Windows and Linux, use `browser` for automated page checks and console output.

On Windows and Linux, ask Cowork to look up a topic or give it a public URL. It can use `web_search` to find pages and `web_open` to read rendered text, follow links and continue through longer pages. The browser uses a fresh profile, reads public HTTP and HTTPS sites, and returns page content and source URLs to the conversation.

In **Settings → Sandbox → Model browser**, enter the full path to Edge, Chrome or Chromium. On Windows you can also select it with **Browse**. Use **Check browser** to verify that Spindle can launch it and reach DevTools, then **Save browser** to use it on this device. **Use auto-detection** clears the saved path. Spindle checks the saved path first, then `SPINDLE_BROWSER`, then standard install locations on Windows or `PATH` on Linux. If Spindle can't find or start a compatible browser, this panel provides install links and you can check again after installing.

Ask Cowork to inspect the console or network traffic for a public page to use `web_open` with `diagnostics: true`. It includes console errors, exceptions and other messages, plus request methods, URLs, resource types, HTTP status, transferred bytes and failures.

Use the side-panel browser for sign-ins and other interactive browsing. Use the separate `browser` model tool to test workspace HTML, run a script against a local preview and inspect its console. **Settings → Sandbox → Block web access for the model browser** disables public page browsing and internet traffic from local page tests.

## Native build outputs

In **Cowork**, ask Spindle to find a tool for the job. For example, “Look up Tauri and Electron for this Windows project, link their documentation, and tell me which tools we already have.” Spindle can search the public web without an API key, inspect the local tool inventory, and look up packages in npm, PyPI and GitHub. Web results include titles, links and excerpts. Set `SPINDLE_SEARXNG_URL` to the search endpoint of a SearXNG instance with JSON output enabled to use it in place of the default Bing RSS search.

When work needs permission, the model can request it in the chat. If you deny an action, Spindle honors that denial for the current request. If a setting blocks an action, the model is given the setting and where to change it. Cowork also checks an unsupported claim of inability once when the model has tried no tools.

When a tool is missing, Spindle can explain its choice and show an approval card with the source, pinned version or release asset, and purpose. On Windows and Linux it can load npm CLIs with a compatible Node installation and registry-only npm controls (npm 11.16 or newer), or portable GitHub executables and ZIPs with a published SHA-256 digest. Python wheels use the existing package installer. Loaded CLIs run in the checked sandbox. Packages needing install scripts, system SDK installation, or an unsupported archive format require another setup method.

Open **Run on** under the chat box to choose an execution device for Cowork. Set a default, or choose separate devices for native builds, Python scripts, installed tools, and Android APKs. For example, run native builds on Windows, Python on Linux, and Java APK builds on Android. In the project page, **Where work runs** sets the defaults for its conversations on this device. A conversation can override those settings.

On the destination, open **Settings › Sync › Devices › Manage** for the source device and enable **Allow sandboxed jobs from this device**. Select the kinds of jobs that device can run. **Check capabilities** in Run on asks the host what it can execute. The response appears in **Work queue › Remote jobs**, alongside other job statuses and **Save output** for completed builds.

Spindle shows the destination and files in an approval card before sending a model-requested job. File edits and model selection stay with the conversation. Pool members can read submitted files and results. Changes to Run on apply to new tool requests. Requests already approved retain their destination. If a selected host is unavailable, Spindle reports the problem so you can change the route.

For an explicit one-off job, `/remote inventory DEVICE` checks available tools, `/remote tool DEVICE TOOL_ID COMMAND [INPUT_FILES] -- [ARGS]` runs an installed CLI, and `/remote build DEVICE main.rs` compiles and tests selected source files. `/remote status` lists jobs and `/remote save JOB_ID` exports a returned executable. Use an explicit Python entry script for remote tests. Unittest discovery and retesting a locally built executable use the local device.

On Windows x64 and Linux x64 or ARM64, set up Python and pass the sandbox check in **Settings → Sandbox**, then install Zig under **Native builds** for C/C++. Rust source builds use an installed stable host Rust toolchain and its standard library. Spindle verifies the pinned toolchain before installing it in its own data folder.

A model can create Rust, C or C++ sources and call `build_binary` to compile and smoke-test a console program. The approval card lists the staged source files and test arguments. Compilation and testing use separate temporary work folders with network isolation and time limits. The test gets a fresh copy of the binary. Its exit status and output go back to the model. `test_binary` reruns a stored binary from the same chat. Each operation needs one-time approval.

Executables that pass go to **Build outputs** in the chat. Those files stay local. Use **Export…** to save a copy outside Spindle. A passing smoke test records how the program behaved for those arguments.

Spindle on Android can edit source, open synced projects, and build small Java Android apps from chat. Ask for an APK using Java and Android framework APIs. Approving the build compiles the named files in an isolated Android service, signs the debug package with Android Keystore, and attaches the verified APK to the chat. **Download** exports it. The built-in profile uses a generated manifest, a launcher Activity, and programmatic UI. Projects with Kotlin, Gradle dependencies, or resource compilation can use a compatible Android toolchain.

### Build a Java/XML APK on the phone

Termux can build a Java APK with an Android platform JAR, `javac`, D8, `aapt2`, `zipalign`, and `apksigner`. The repository helper [build-termux-apk.py](../tools/build-termux-apk.py) uses that pipeline. Copy the source project and helper to Termux, then point it at the installed platform JAR and the project's manifest minimum API.

```sh
python3 build-termux-apk.py ~/daytasks-apk \
  --android-jar ~/daytasks-apk/android30.jar --min-api 24 \
  --output ~/storage/downloads/DayTasks.apk
```

The project root contains `AndroidManifest.xml`, `src/**/*.java`, and optional `res/`. Both the toolchain and the platform jar must already be available. To write to Downloads, grant Termux shared storage access with `termux-setup-storage`. The helper verifies the signed APK before publishing it. Open it from Downloads to install. Keep the same debug keystore and increment the manifest version code for updates to an installed app.

Use this helper for dependency-free Java/XML apps that do not reference a generated `R.java`. Gradle, Kotlin, and projects with dependencies use their existing build workflows. Run the helper directly in Termux with sources you trust. Spindle's Android app has no connection to the Termux shell.

The [built-in Java builder prototype](../android/builder/README.md) exercises the Termux-free path in a separate development APK.

Use `/android-apk <task>` or open **Skill library → Built-in → /android-apk** for guidance on the Termux workflow, the built-in diagnostic, and existing Gradle projects. APK requests can also load the reference automatically. Spindle chooses a build route from the tools the session can actually use.

Spindle's file-editing tools write plain text. Models can use the Python sandbox to generate formats such as `.docx`, `.xlsx`, `.pptx`, and `.pdf` when the required packages are available.

A paste of at least 4,096 UTF-8 bytes or 40 lines becomes a removable file chip in the composer. Click it to review the full text. **Send** writes the text to an ordinary file in the project or scratch artifact folder. Project folder access applies to those files. The selected model receives the full text when it fits, or labeled beginning and ending excerpts. A message can contain up to eight pasted files totaling 2 MiB. The thread retains a snapshot and refers to the file by name on subsequent turns.

**Settings → Preferences → Context compaction** sets the workspace default. A model's **Advanced** settings can override it for that model, and **Conversation settings** can override it for a single thread. **Aggressive**, **Normal** and **Generous** compact at 25%, 50% and 75% of the model window. **Dynamic** is generous while a model has plenty of its usage allowance and compacts harder (down to 20%) as it nears a limit set in **Automations → Usage limits**. Without a limit it behaves like Normal. **Fine-tune** sets a custom percentage or a token ceiling. The estimate counts input, including the upcoming prompt. Spindle checks before sending a conversational or planning request and uses a 4,096-token estimate if the model's window is unknown. A local, speaker-attributed summary carries older decisions and file references alongside recent turns. **/compact** runs it manually and **/compact status** shows the active policy. The full transcript remains available.
