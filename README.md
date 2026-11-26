# Spindle

A native Rust desktop workspace for conversations, projects, and multi-model reviews. Open **Start Spindle.cmd** in this folder. Ollama on localhost port 11434 works without an API key. The default is `qwen3.5:9b` when available. The older launcher also opens Spindle.

## Providers and settings

Use **Settings → Model providers → Add provider** to test and save an LM Studio, OpenAI, OpenRouter, Anthropic, or custom Chat Completions connection. Enter the provider's API base URL, including its API version path, and supply a key if required. Spindle fetches the model list. Windows Credential Manager stores keys. Selecting a hosted model sends its request context to that provider. Accounts and billing are managed with the provider.

Click the model control beside the message box or press **Ctrl+K**. Search by model or provider, star favorites, and select a result. Icons beside each model show what it accepts beyond text: an eye for images and sound waves for audio. A brain marks models that can reason. Spindle reads these from Ollama's reported capabilities and provider catalogs, and otherwise from known model families. They describe the model; Spindle does not yet send images or audio.

**Settings → Preferences** controls the accent color (Red, Orange, Yellow, Green, Blue, Indigo, Violet or Magenta), conversation text size, the send shortcut, context compaction and the council roster. The native Windows caption follows the charcoal application theme.

The sidebar, welcome screen and window icons follow the workspace accent. The Windows executable uses the default blue ribbon. [View all eight icons](assets/accents/palette.png)

### Choosing which models appear

**Settings → Models** lists every connected model with a search box and a provider filter. Clear a model's checkbox to hide it from the picker. **Show all listed** and **Hide all listed** act on the current search and filter, so you can hide a provider's whole catalog and keep a few. Hidden models stay connected, and work already using them continues.

### Thinking

For models that can reason, a brain button beside the model picker shows the thinking level, for example **Auto** or **High**. Click it to open a stepped slider. When Ollama reports a model's own levels through `/api/show`, the slider uses exactly those. Otherwise it offers Off, Low, Medium and High. **Reset** returns to the model's default. The level applies to that model in every chat.

### Advanced model settings

Each row in **Settings → Models** has an **Advanced…** button. It sets compaction, thinking, temperature and, for local Ollama models, the context window for that one model. These change how the model behaves everywhere and are meant to be sought out deliberately. **Reset to defaults** clears them. A model with custom settings is tagged in the list.

### Model connections

The picker groups **Ollama** and **Ollama Cloud** separately. Both use the local server on port 11434. Cloud tags and remote aliases use the account already signed in to Ollama. **Maya** uses its OpenAI-compatible model catalog. Its preset points to `http://127.0.0.1:18081/v1`. Refresh that provider after switching Maya profiles.

In **Settings › Model providers › Find local AI servers**, leave the host blank to check this computer or enter the name or IP of a LAN host. Spindle checks standard API ports for Ollama, LM Studio, Maya, ComfyUI, Stable Diffusion WebUI, and OpenAI-compatible services such as llama.cpp, LocalAI and vLLM. Choose **Connect** for a model server or **Use for images** for an image server. Use **Add provider** for other ports or API credentials. From a terminal, `spindle --discover [HOST]` lists available servers.

In **Settings → Model providers → Add provider**, choose **OpenAI · Sign in with ChatGPT** on Windows or Android for eligible ChatGPT plan models. The alphabetical provider list and connection form open in Settings, with **Back** to return. Spindle opens the system browser, uses PKCE, validates the signed identity token, and stores renewable credentials using Windows account protection or Android Keystore. **OpenAI API** connects to the Responses API with an API key. [OpenAI sign-in and plan usage](https://developers.openai.com/siwc/token-sharing-open-source)

On Windows or Linux, choose **Claude · Console OAuth**, select the official Anthropic `ant` executable, and choose **Continue with Claude Console**. The CLI manages its own local login profile, and API usage is billed to the Console workspace you authorize. **Claude · Anthropic API** accepts a Console API key on Windows and Android. Anthropic limits Claude subscription sign-in to Anthropic apps. [Claude Console CLI authentication](https://platform.claude.com/docs/en/cli-sdks-libraries/cli/authentication) · [Claude subscription credentials](https://code.claude.com/docs/en/legal-and-compliance)

## Schedules, fallback and usage

Under **Automations → Model fallback**, enable **Also use fallbacks when the provider disconnects or times out** to try the next eligible model on a connection failure. This is off by default. Each queued request uses its approved fallback list and skips models that already disconnected on that request. If a goal stops after planning, choose a model in the composer and select **Continue goal** on the failure message to use the saved plan. The equivalent commands are `/fallback disconnects on` and `/goal continue`.

Use **Automations → Usage limits** or `/usage` to view Ollama Cloud’s seven-day account requests, included and purchased balances when supplied, and quota windows with reset times. Spindle queries the signed-in local Ollama service on startup, every five minutes, and on refresh. The hold percentage applies to cloud windows across devices. OpenAI and Anthropic connections show limits returned in response headers and model-level token counts for work sent through this workspace. [Ollama API specification](https://github.com/ollama/ollama/blob/main/docs/openapi.yaml)

Open **Automations → Schedules** to give a task a prompt, destination chat, model, first local run time and optional repeat interval. An overdue repeat runs once and advances to its next interval. The answer goes back to the original chat even if another chat is open. Use **Defer current task** or `/defer YYYY-MM-DD HH:MM` to stop the current attempt and resume the task later with its prompt and steering intact.

For a recurring skill, enter `/skill-name assignment` as the prompt. Spindle captures the skill definition and holds the run for review if the skill changes.

**Run due tasks while Spindle is closed** registers a per-user Windows Task Scheduler entry for this workspace. Windows checks once a minute while the user is signed in and the computer is awake. The desktop handles due work while it holds the workspace session lock. A process interruption leaves in-flight work held for review. Each collaborator configures their own schedules.

In **Model fallback**, add models in the order in which they may receive your prompt and chat history. New tasks and schedules capture that order. **Usage limits** supports a five-hour and a weekly request budget for a provider or model. Enter the allowance, current use, threshold percentage and next reset. Choose **Try fallback** or **Pause until reset** for each budget. If every eligible route is limited, Spindle holds the task and retries at the earliest eligible reset. An unknown provider reset stays held until cleared, or uses a matching configured budget reset for the next check. The activity tab records switches and holds. Explicitly selected reviewers and workers keep their models, and attachment approval stays within its provider.

Spindle counts dispatches made in this workspace. It displays token counts returned by providers and acts on supported quota headers and HTTP 429 responses. Other apps can consume the same account allowance. Ollama Cloud may return token counts without reporting a remaining account percentage or reset.

## Document reader

Select an excerpt directly in the conversation by dragging across messages, then press **Ctrl+C**. On a touch screen, hold a word, drag to the end of the passage, and tap **Copy selection**. Hold near the top or bottom edge to keep scrolling.

Press **Ctrl+B** to collapse or expand the sidebar. Open **Reader**, press **Ctrl+O**, or drop a document into the window. Drag the reader's left edge to resize it. The reader supports Markdown (rendered, with tables), Word DOC/DOCX, RTF, CSV/TSV, Excel XLS/XLSX/XLSM/XLSB, PDF, HTML and plain text. Workbooks are organized by sheet and row range. PDFs offer page images and extracted text where available. HTML offers extracted text and an interactive preview. Legacy DOC uses `antiword` and PDF text uses `pdftotext` when installed. Windows renders PDF page images directly.

Opening a document previews it locally. **Excerpt to chat** attaches up to 6,000 UTF-8 bytes of a workspace file to the next request after you approve it. The reader accepts files up to 32 MiB and bounds extracted content for the desktop preview.

**Edit** opens Markdown, text, CSV/TSV, Word, Excel (XLSX) and RTF files for editing in the side panel. Markdown has Edit and Preview tabs, a formatting toolbar and word and line counts. Spreadsheets open as a grid with sheet tabs. Right-click a row number or column letter to insert or delete rows and columns. **Save** writes over the original and refuses if the file changed on disk since you opened it. **Save as** always writes a new file. Spindle's Word and Excel writer keeps text and cell values only, so saving over a .docx or .xlsx first warns that fonts, styles, formulas, charts and images will be lost. RTF, XLS, XLSM, XLSB and DOC save only as a new file.

## Projects

The sidebar's **Projects** section lists your projects with their chats nested beneath them, above your standalone **Chats**. Click a project's name to open its page, click its folder icon to show or hide its chats, and use its **+** for a new chat in that project. The **+** on the Projects header creates a project.

**Create a project** asks what you are working on and what you want to achieve, with optional instructions. The folder defaults to `Documents\Spindle Projects\<name>`. **Change** picks another location. A project's page shows its goal, a **New chat in …** box, its conversations, and controls for **Instructions**, **Name and goal**, **Files** (opens the artifacts folder), the folder path, **Sync and sharing** and **Delete project**. The project's goal and instructions are added to every chat in it. Files generated in a project chat go to that project's `artifacts` folder.

**Delete project** permanently removes the project's conversations, including their queued work and schedules, and removes the project from Spindle. Paired devices apply the deletion when they sync. You must type the project's name to confirm. The folder and its files stay on disk.

Project files follow the folder's access permissions. Open a shared Windows folder from **Sync and sharing → Open shared folder…**.

**Encrypted thread sync** pairs devices through that project folder. On each device, open the same LAN folder or a replicated copy that includes the hidden `.spindle` directory. Enter the same project passphrase. Enabling sync shares all existing and future threads in this project. Each completed turn and pending-work checkpoint becomes an authenticated, encrypted revision. Devices check for changes approximately every eight seconds. Offline edits that diverge appear as separate threads. A running task continues on its originating device. Receiving its checkpoint never dispatches another model request.

For a phone, choose **Export phone companion** in Projects. Open the exported HTML in a browser with Web Crypto on a trusted HTTPS origin or a supported local file. Choose the project metadata folder, or select its `sync-v1.json` and `thread-*.spindle` files together. Unlock it to read threads and prepare a draft. With a file picker, save the downloaded encrypted revision back to the same metadata folder. In desktop Spindle, open that thread and choose **Use draft in composer**. The companion keeps its key in memory and processes the selected files on the device. Inference runs in the desktop app.

Spindle encrypts the full local session for the signed-in Windows user, including queued prompts and attachment snapshots. Opening an earlier session migrates it atomically. Shared revisions use a random project salt, PBKDF2-HMAC-SHA256 (600,000 iterations) and AES-256-GCM. Keep the passphrase for recovery. Anyone with the passphrase and revision files can read the synced conversations. Credentials and executable queues stay on the originating device. Earlier explicitly published plaintext snapshots remain available under **Earlier plaintext shares**. Project output files, OS backups and copies of earlier sessions have their own storage protections.

## Threads and generated files

Sending the first message adds **New chat** to the sidebar immediately. The first completed reply supplies its title. A name you choose while the model is working takes precedence.

Click **…** beside any thread, or right-click it, to expand its options in the sidebar. You can rename, pin, mark unread, move to a project, branch, read, copy, archive, or delete a conversation. **Conversation settings** opens a panel for its title, next model, section, project, and compaction. **Back** or **Close** returns you to what you were viewing. **Branch** starts an independent conversation with the same history. Archived threads are available through **Archive** in the sidebar. Moving a chat changes where its new files are saved. Files already generated stay where they are.

**Delete conversation** asks for confirmation, removes the thread and its queued work and schedules, and propagates the deletion when project sync runs. Earlier encrypted revision files, independent branches, generated files and backups have their own retention. Re-export the phone companion from Projects to use the updated thread list.

The composer shows the latest reported context usage, the active model’s context window when available, and generation speed. Streaming speed uses a visible-text estimate. Final speed uses provider timing when available or reported output tokens divided by elapsed request time. The model answering a new chat supplies its title in the same response. For models that omit a title, Spindle uses the first line of the prompt. Each thread remembers the selected model, and individual responses retain their actual answering model.

Spindle checks the selected model's published output and context limits when available. Ollama and Claude requests start with up to 16,384 output tokens, with room reserved for the prompt in a known shared context. Ollama Cloud uses the hosted context window. Other providers can manage their own output budget. When a model reaches its output limit, Spindle saves the partial reply in the encrypted local queue and requests the missing text. Every continuation passes the same usage and fallback checks. Generated files are saved after the reply finishes. Repeated cutoffs can still pause a long task, and **Resume** continues from the saved text.

Your prompts appear on the right and model replies on the left. For file deliverables, ask for a file such as “Create a Snake game in snake.html”, or name a format (“as a .csv”). Spindle saves the output in the project's **artifacts** folder or the thread's scratch folder in Windows Temp. Each file appears as a card where the reply placed it, with its name, type and size, a primary **Open** (or **Preview** for HTML), and **More** for View source, Edit, Download, Show in folder, Copy path and Save a copy. A file name mentioned in the reply is a link that opens it. Markdown, spreadsheets, Word, RTF and PDF open in the reader. HTML runs in a sandboxed WebView2 in the resizable right panel. Code blocks that stay in the message have Copy and Save file. **/preview <file>** opens a workspace file in the same panel.

### Revising work in progress

Switch between **Chat** and **Cowork** in the sidebar. The sync indicator sits beside Settings. Open **Panel → Plan** to edit the current conversation's checklist. Choose **Revise plan…** to describe a change while the model is working. Check **Replace the current objective** when the deliverable itself has changed. For example, `/goal replace Build and test a Windows Rust executable` replaces an HTML goal. Use `/plan revise Include an empty-input test` to keep the objective and amend the plan. Steering an active plan also carries your correction into subsequent tool turns.

In a project chat, ask about a previous discussion or name a distinctive topic from it. For example, “What build flags did we agree on in the previous thread?” Chat and Cowork can draw on short excerpts from other chats in the same project and cite the source chat. This works with Project Memory off and includes project chats already synced to this device.

### Native build outputs

In **Cowork**, ask Spindle to find a tool for the job. For example, “Look up Tauri and Electron for this Windows project, link their documentation, and tell me which tools we already have.” Spindle can search the public web without an API key, inspect the local tool inventory, and look up packages in npm, PyPI and GitHub. Web results include titles, links and excerpts. Set `SPINDLE_SEARXNG_URL` to the search endpoint of a SearXNG instance with JSON output enabled to use it in place of the default Bing RSS search.

When work needs permission, the model can request it in the chat. If you deny an action, Spindle honors that denial for the current request. If a setting blocks an action, the model is given the setting and where to change it. Cowork also checks an unsupported claim of inability once when the model has tried no tools.

When a tool is missing, Spindle can explain its choice and show an approval card with the source, pinned version or release asset, and purpose. On Windows and Linux it can load npm CLIs with a compatible Node installation and registry-only npm controls (npm 11.16 or newer), or portable GitHub executables and ZIPs with a published SHA-256 digest. Python wheels use the existing package installer. Loaded CLIs run in the checked sandbox. Packages needing install scripts, system SDK installation, or an unsupported archive format require another setup method.

Paired hosts accept remote jobs after you enable **Allow sandbox jobs from this device** in their Sync settings. Use `/remote devices`, then `/remote inventory DEVICE` to inspect a target. Use `/remote tool DEVICE TOOL_ID COMMAND [INPUT_FILES] -- [ARGS]` for an installed CLI, or `/remote build DEVICE main.rs` to compile and test sources. The sync pool carries the selected text files and result. Use `/remote status` to see jobs and `/remote save JOB_ID` to export a returned executable. Every pool member can read submitted files and results.

On Windows x64 and Linux x64 or ARM64, set up Python and pass the sandbox check in **Settings → Sandbox**, then install Zig under **Native builds** for C/C++. Rust source builds use an installed stable host Rust toolchain and its standard library. Spindle verifies the pinned toolchain before installing it in its own data folder.

A model can create Rust, C or C++ sources and call `build_binary` to compile and smoke-test a console program. The approval card lists the staged source files and test arguments. Compilation and testing use separate temporary work folders with network isolation and time limits. The test gets a fresh copy of the binary. Its exit status and output go back to the model. `test_binary` reruns a stored binary from the same chat. Each operation needs one-time approval.

Executables that pass go to **Build outputs** in the chat. Those files stay local. Use **Export…** to save a copy outside Spindle. A passing smoke test records how the program behaved for those arguments.

Spindle on Android can edit source, open synced projects, and build small Java Android apps from chat. Ask for an APK using Java and Android framework APIs. Approving the build compiles the named files in an isolated Android service, signs the debug package with Android Keystore, and attaches the verified APK to the chat. **Download** exports it. The built-in profile uses a generated manifest, a launcher Activity, and programmatic UI. Projects with Kotlin, Gradle dependencies, or resource compilation can use a compatible Android toolchain.

### Build a Java/XML APK on the phone

DayTasks was built in Termux with an Android platform jar, `javac`, D8, `aapt2`, `zipalign`, and `apksigner`. The repository helper [build-termux-apk.py](tools/build-termux-apk.py) uses that pipeline. Copy the source project and helper to Termux, then point it at the installed platform jar and the project's manifest minimum API.

```sh
python3 build-termux-apk.py ~/daytasks-apk \
  --android-jar ~/daytasks-apk/android30.jar --min-api 24 \
  --output ~/storage/downloads/DayTasks.apk
```

The project root contains `AndroidManifest.xml`, `src/**/*.java`, and optional `res/`. Both the toolchain and the platform jar must already be available. To write to Downloads, grant Termux shared storage access with `termux-setup-storage`. The helper verifies the signed APK before publishing it. Open it from Downloads to install. Keep the same debug keystore and increment the manifest version code for updates to an installed app.

Use this helper for dependency-free Java/XML apps that do not reference a generated `R.java`. Gradle, Kotlin, and projects with dependencies use their existing build workflows. Run the helper directly in Termux with sources you trust. Spindle's Android app has no connection to the Termux shell.

The [built-in Java builder prototype](android/builder/README.md) exercises the Termux-free path in a separate development APK.

Use `/android-apk <task>` or open **Skill library → Built-in → /android-apk** for guidance on the Termux workflow, the built-in diagnostic, and existing Gradle projects. APK requests can also load the reference automatically. Spindle chooses a build route from the tools the session can actually use.

Spindle's file-editing tools write plain text. Models can use the Python sandbox to generate formats such as `.docx`, `.xlsx`, `.pptx`, and `.pdf` when the required packages are available.

A paste of at least 4,096 UTF-8 bytes or 40 lines becomes a removable file chip in the composer. Click it to review the full text. **Send** writes the text to an ordinary file in the project or scratch artifact folder. Project folder access applies to those files. The selected model receives the full text when it fits, or labeled beginning and ending excerpts. A message can contain up to eight pasted files totaling 2 MiB. The thread retains a snapshot and refers to the file by name on subsequent turns.

**Settings → Preferences → Context compaction** sets the workspace default. A model's **Advanced** settings can override it for that model, and **Conversation settings** can override it for a single thread. **Aggressive**, **Normal** and **Generous** compact at 25%, 50% and 75% of the model window. **Dynamic** is generous while a model has plenty of its usage allowance and compacts harder (down to 20%) as it nears a limit set in **Automations → Usage limits**. Without a limit it behaves like Normal. **Fine-tune** sets a custom percentage or a token ceiling. The estimate counts input, including the upcoming prompt. Spindle checks before sending a conversational or planning request and uses a 4,096-token estimate if the model's window is unknown. A local, speaker-attributed summary carries older decisions and file references alongside recent turns. **/compact** runs it manually and **/compact status** shows the active policy. The full transcript remains available.

## Review council

Use `/council` to get independent feedback on the latest response, or `/council <question or material>` to focus the review. An approved attachment can supply the material. The council calls its saved reviewers sequentially, with no execution tools, and returns each model's feedback under its own heading. Use `/council roster` to inspect the panel and **Settings → Preferences** to change it. The initial roster prefers local `qwen3.5:9b` and `Maternion/spark-x2.6:4b` when installed.

## Working in the desktop app

- Start a conversation, select a model, and type a message. Enter sends it and Shift+Enter inserts a line break. Responses appear as Ollama generates them. Recent chats reopen from the sidebar.
- Choose **Draft a plan** or enter `/plan <task>`. Use `/plan revise <change>` and `/plan accept <revision>` to work from a specific draft. `/goal <objective>` sets a continuing objective for the chat.
- While a response is running, **Queue** adds a follow-up and **Steer** applies a correction to the active request. **Stop** cancels inference and holds remaining work. The Work queue lets you remove or promote a waiting message. After an interrupted session, **Resume** releases the saved queue.
- **Ask two models** lets you choose each worker's role and model. Workers run sequentially and the coordinator synthesizes their findings. From any chat, use `/swarm Check boundary conditions --worker reviewer=qwen3.5:9b` for one worker or append a second `--worker role=model`. Use `/steer --worker reviewer <correction>` for one role.
- **Attach file** previews a workspace file for approval. The harness sends the approved snapshot with the next submitted task. The native picker supports UTF-8 text files up to 6,000 bytes. You can ask for analysis or a proposed patch.
- In **Skills**, create a named draft from a brief, inspect its Markdown, try an example, and install it. Invoke it as `/<name> <task>`. The first-pass package format is one UTF-8 `SKILL.md` with simple front matter containing `name` and `description`, followed by its instructions. Installing from the workspace copies that definition into the local registry. Reinstall to update.

## Build and CLI

Use Rust 1.89 or later and the MSVC C++ build tools on Windows. Start Ollama and load at least one local chat model into its catalog.

```powershell
cargo test --locked
cargo run --release --locked
cargo run --release --locked -- --cli
cargo run --release --locked -- --models
cargo run --release --locked -- --ask qwen3.5:9b "Reply with two words."
```

While Spindle is running, rebuild into a separate folder with `cargo build --release --locked --offline --target-dir target/staged`. **Start Spindle.cmd** picks the newest build.

On Windows and Linux, **Settings › Preferences › Updates** checks the project's GitHub Releases. Choose **Download update** to stage the version for your platform. Spindle verifies the published SHA-256 digest again when you exit and reopen it. The desktop release workflow publishes assets for a Git tag that matches the version in `Cargo.toml`.

The current working directory identifies the workspace, and each workspace keeps its own chats. Start Spindle from the same folder each time (for example with **Start Spindle.cmd**). Starting the executable from its build folder opens a different, empty workspace. Projects open inside the one window from the sidebar. Desktop and interactive CLI share one session lock per workspace. Chats, pending work, attachment snapshots, and installed skills are saved in a Windows-user-encrypted session under `%LOCALAPPDATA%\Spindle\state`. One-shot queries use no session file.

## First-pass scope

The built-in Ollama connection streams through the local daemon for both local and cloud models. The one-shot model flags use that same connection. Conversations use a bounded window of recent exchanges. Scheduled tasks have the same conversational capabilities as interactive tasks. Applying patches, running repository commands and tests, OS containment and remote skill installation remain later milestones in [the design](Spindle-Design.md).

## Sync across devices

Settings › Sync keeps conversations, projects, settings, skills, memories and API keys the same on your devices, end-to-end encrypted (servers and shared folders only hold encrypted records). Sign-ins like ChatGPT's stay per device.

- **Start:** Settings › Sync › Start syncing, then add your other devices. Devices on the same network find each other and sync directly (TCP 47613, UDP 47614; allow Spindle through the firewall on private networks), with no server or folder. If one isn't found, add it with `/sync peer IP:port` or Settings › Sync. A shared folder or a sync server also reaches devices that aren't on the network. Run your own server with `spindle sync-server --data DIR --admin-token TOKEN [--mode store|relay|both] [--mailbox-hours N]`.
- **Device name:** defaults to the computer's name; change it with Rename on Settings › Sync, or `/sync rename NAME`. Other devices see the new name on their next sync.
- **Add a device:** Add a device shows a QR code and a short code; on the new device choose Join and paste the link, or type the code with the server's address. The new device then shows a number from 1 to 100; type it on the existing device to finish (three wrong numbers end the pairing). That way, someone who guessed the code still needs to see the new device's screen. With an account saved on your server, a new device can also join by signing in.
- **Branches:** a chat continued on two devices at once keeps the line of the device where it started as the main branch; the other device's turns appear as a branch you can view, make main or delete.
- **What syncs:** conversations (on) and project files (off) are defaults each project can override. Use **Installed skills** to control skill sync for this device. **Providers without credentials** is off by default and requires opt-in on both devices. Shared loopback endpoints use the sending device's hostname and keep their original port and path. Use **Hostname for providers on this device** if the default name cannot be reached on your network.
- **Project sync:** **Stop sync** pauses publication and import for the project across the pool and keeps local copies. **Resume sync** lets the devices catch up. Use **Delete project on all synced devices** in the confirmation panel for a pool-wide deletion. **Also delete project files on all devices** starts unchecked. Offline devices apply the deletion when they reconnect. Persistent project IDs let a device reuse an existing copy after a rename or re-pair.
- **Devices:** to retire an old installation, use **Settings → Sync → Devices → Remove device** on a remaining device. Removed devices stay under **Show removed devices** for reference.
- New projects carry a UUID in `.spindle/sync-id.json`. Older projects keep their saved sync IDs. To join a confirmed legacy duplicate, use `/sync link-projects <duplicate ID> <original ID>`. Spindle groups the chats under the original project and keeps both folders on disk. Paired devices learn the link on their next sync.
- **Unread:** a dot marks a conversation with new messages or branch updates you haven't viewed on this device. Open the conversation to clear it.
- **In the background:** "Run Spindle in the background" starts Spindle when you sign in and keeps it in the tray when you close the window. One click on **Exit Spindle** in the sidebar or tray closes the window and stops this workspace's engine and tray. **Ctrl+Q** and `spindle service stop` do the same. Queued work is held for later. Manage startup with `spindle service install|uninstall|status`.
- Command line: `spindle sync [now | status | devices | create NAME | folder PATH | server URL [ADMIN] | account USER PASSWORD | join-account URL USER DEVICE PASSWORD | pair | pair NUMBER | join LINK-OR-CODE [WHERE] [NAME] | rename NAME | peer ADDRESS | remove DEVICE | switch NAME on|off | project DIR conversations=… files=… hidden=…]`.

## Android

`android/Spindle.apk` is Spindle for Android phones (64-bit ARM, Android 8 or newer). It is the same program as the desktop app, laid out in one column: the sidebar opens from the menu button at the top left, and side panels cover the conversation until you close them.

- **Install:** copy the APK to the phone and open it (allow installing from that app when Android asks), or `adb install -r android/Spindle.apk`.
- **Set up:** Settings › Sync › Join, then pair with a desktop (Add a device there, type its code on the phone, type the phone's number on the desktop). On the same Wi-Fi the phone syncs with your desktops directly.
- **Models:** add your model servers by address (for example `http://model-host.local:11434` for Ollama); `localhost` on the phone means the phone itself.
- **Microphone:** grant it in Android Settings › Apps › Spindle › Permissions; the app can't ask yet.
- **What's different:** Android offers isolated Java APK builds. The desktop Python and C/C++ sandbox, built-in browser, and pasting images are desktop features. Spindle syncs while the Android app is open.
- **Build:** Run `android/build-apk.sh` in Linux or WSL with a JDK, Python 3, Android SDK 34, Build Tools 34 and 35, NDK 27, and `rustup target add aarch64-linux-android`. Set `SPINDLE_ECJ_JAR` to the ECJ 4.12 jar used by the [build probe](android/builder/README.md). Keep `android/spindle-development.keystore` for updates to this package. The renamed Android app installs separately from earlier builds. Sync shared data from the earlier app before removing it.

## Images (ComfyUI)

Set the ComfyUI address in **Settings › Models › Image generation**, or use `/image server http://model-host.local:8188`. Use **Test** to list checkpoints from the server.

- `/image a red fox in the snow` generates a PNG. Models can also generate pictures with the `image` tool.
- Attach a PNG or JPEG and send `/image edit make the sky violet`, or use `/image edit "red fox.png" -- make the sky violet` with a workspace file or an image created in this chat. The edit saves a separate file.

Both commands have built-in SDXL workflows in ComfyUI. To customize one, use **Workflow › Export (API)** in ComfyUI and set the corresponding workflow path in Spindle. Templates accept `{{prompt}}`, `{{negative}}`, `{{seed}}`, `{{width}}` and `{{height}}`. An edit workflow also needs `{{image}}` as an input value, such as the **Load Image** node's image field. You can also select Stable Diffusion WebUI with its API enabled. That backend uses its currently loaded checkpoint. The selected image server receives prompts and edit source files.

## Audio

Spindle talks and listens through Ollama when you have a model that can (for example `ollama pull lfm3-omni`). Use the microphone button in the message box to dictate, or the speaker button on a reply to hear it. Attach a recording (.wav, .mp3, .m4a, …) and Spindle writes down what is said and attaches the transcript. `/speak some text` saves a WAV in the chat, and models can use `speak` and `transcribe` when you ask. Settings › Models › Audio picks the models and voice, turns on reading every reply aloud, or points Spindle at another server with the OpenAI audio API. You can also use Fish Audio (paid, many voices) for speaking, transcribing or both: choose it there and add your API key.

## Side panel

The Panel button opens the right-hand panel: **Viewer** (documents and previews), **Files** (what you attached to this chat and what it made), **Library** (every file in the chat's project or the workspace) and **Browser** (a web page beside the chat on Windows; other systems open your own browser).

### Web browsing in Cowork

Browser Link uses the Edge or Chrome profile you already use, with its open tabs and sign-ins. The built-in browser in Spindle is isolated. Choose your browser in the chat or in **Settings › Computer use**. Spindle verifies and unpacks the extension, registers its local connection, and opens the install guide. On Windows, the extension is under **Downloads**. In your browser’s **Manage extensions** page, turn on **Developer mode**, choose **Load unpacked**, and select the Browser Link folder. Spindle continues the chat when the extension connects. A bundled copy is available offline. [Extension guide](browser-extension/README.md)

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

## Using Spindle from other programs (CLI and MCP)

Whichever Spindle has a workspace open (the window, or a windowless one) also answers other
programs on the same computer. If none is running, the command starts a windowless Spindle,
which stops after five minutes with no requests and no work, and hands the workspace over
when you open the window. Chats started this way appear in the normal chat list.

Commands run in the workspace folder (or pass `--dir`); add `--json` for scripts:

```
spindle ask "Make form.html: a contact form" [--project DIR] [--model qwen3.5:9b] [--goal]
spindle send 12 "Add a phone field"
spindle status [12] [--wait 120]
spindle chats · spindle show 12 [--full] · spindle file 12 form.html [--save out.html]
spindle approve 34 [once|conversation|project] · spindle deny 34   (at a terminal only)
spindle projects · spindle models
```

Anything that needs approval waits for you in the Spindle window, or at a terminal with
`spindle approve`. Scripts and agents can't approve (the command refuses without a terminal,
and MCP has no approve tool).

### MCP (Claude Code, Codex, VS Code, other agents)

`spindle mcp --dir WORKSPACE` is an MCP server on stdio with eight tools: `spindle_ask`,
`spindle_send`, `spindle_status`, `spindle_read_chat`, `spindle_list_chats`,
`spindle_list_projects`, `spindle_list_models`, `spindle_get_file`. WORKSPACE is the folder
Spindle runs in (for the desktop shortcut, this folder).

Claude Code:
```
claude mcp add spindle -- "C:\Projects\Spindle\target\release\spindle.exe" mcp --dir "C:\Projects\Spindle"
```
Codex (`~/.codex/config.toml`):
```toml
[mcp_servers.spindle]
command = 'C:\Projects\Spindle\target\release\spindle.exe'
args = ["mcp", "--dir", 'C:\Projects\Spindle']
```
VS Code (`.vscode/mcp.json`):
```json
{ "servers": { "spindle": { "type": "stdio",
  "command": "C:\\Projects\\Spindle\\target\\release\\spindle.exe",
  "args": ["mcp", "--dir", "C:\\Projects\\Spindle"] } } }
```
If the build script put the newest build in `target\staged\release`, use that path, or copy
`spindle.exe` somewhere fixed and point at that.

How it works: the owning Spindle listens on 127.0.0.1 (a random port) and writes the port and
a random token to `%LOCALAPPDATA%\Spindle\control\<workspace>.json` (Linux:
`~/.local/share/Spindle/control/`, mode 0600). Requests must carry the token.
