# Using Spindle

[Spindle](../README.md)

## Working in the desktop app

- Start a conversation, select a model, and type a message. Enter sends it and Shift+Enter inserts a line break. Responses appear as Ollama generates them. Recent chats reopen from the sidebar.
- Every chat can use Spindle's workspace and image tools. Tool permissions still apply.
- Choose **Draft a plan** or enter `/plan <task>`. Use `/plan revise <change>` and `/plan accept <revision>` to work from a specific draft. `/goal <objective>` sets a continuing objective for the chat.
- While the conversation is working, **Enter** steers the next model request and **Ctrl+Enter** queues a follow-up. Swap these shortcuts in **Settings → Preferences → Conversation**. Both messages appear in the chat. An in-progress tool can finish before the model receives your steer. **Stop** cancels inference and holds remaining work. The Work queue lets you remove or promote a waiting message.
- Adjust the per-request tool budget in **Settings → Preferences → Tool step limits**. Conversations default to 30 steps and Bobbins to 12.
- **Ask two models** lets you choose each worker's role and model. Workers run sequentially and the coordinator synthesizes their findings. From any chat, use `/swarm Check boundary conditions --worker reviewer=qwen3.5:9b` for one worker or append a second `--worker role=model`. Use `/steer --worker reviewer <correction>` for one role.
- **Attach file** previews a workspace file for approval. The harness sends the approved snapshot with the next submitted task. The native picker supports UTF-8 text files up to 6,000 bytes. You can ask for analysis or a proposed patch.
- In **Skills**, create a named draft from a brief, inspect its Markdown, try an example, and install it. Invoke it as `/<name> <task>`. The first-pass package format is one UTF-8 `SKILL.md` with simple front matter containing `name` and `description`, followed by its instructions. Installing from the workspace copies that definition into the local registry. Reinstall to update.

## Notes

Open **Notes** in the sidebar and select **New note** to open the editor immediately. Give the note a title before saving it. The toolbar inserts headings, emphasis, lists, links, tables, images, and tags. **Preview** renders Markdown and local PNG or JPEG links. On Windows, **Add image file** copies an image beside the note. External image URLs open when clicked. Use **Save** or Ctrl+S, then **All notes** to see your sticky-note cards.

Choose **Obsidian vault** or **Settings → Connectors → Obsidian** to connect an accessible vault folder. Spindle opens the vault's Markdown files directly. Use **Refresh** to reload a saved note and discover new files. If the note changed on disk while you were editing, Spindle keeps your draft and reports the conflict. Android requires a folder the app can read and write.

Open a chat, then select **Attach to chat** on a saved note. A snapshot of that note goes with your next message. Repeat from another chat to attach the same note there. The Notes library and vault connection live on the current device.

Use **Tag** or type a tag such as `#release/checklist`. Spindle also recognizes `tags` in Obsidian front matter. Ask the model to search Spindle notes or your Obsidian vault by text or by tag. Each search separately asks permission to share matching names and short excerpts with that model. Attaching a note only shares that note's snapshot.

## Threads and generated files

Use **Search chats** in the sidebar or Ctrl+Shift+F to find conversations by title or message text. Project chats and archived chats are included. Click a result to open the conversation.

When you edit and resend a message, the new version keeps the files from earlier messages. In a chat without a project, each version can edit its own copy of those files.

**Settings → Troubleshooting** shows the latest model and tool failures on this device, with a jump back to the chat. For eligible public-page failures, the chat offers **Use regular browser** through Browser Link and asks for normal tab access. **Copy troubleshooting report** groups failures by category and tool. The report omits conversation content, URLs, local paths, and model names by default. You can opt in to including model names.

Start a **2-day capture** or **3-day capture** in Troubleshooting and use Spindle normally. It records failures, Browser Link outcomes, and sync decisions while Spindle is running. A capture survives an app restart and stops at the end of its window. It keeps up to 5,000 events and reports any older events omitted at that limit. The report stays available until you replace the capture. Sync entries distinguish branches inside a chat from conflicts preserved as separate chats. A same-title entry counts chats in the same project and notes how many share a history prefix or import identity.

From the workspace folder on desktop, `spindle --troubleshooting-report` reads the saved report while the app is open. For local investigation, add `--local` to include model names and chat numbers. Each device keeps its own capture. On Android, use **Copy troubleshooting report** to bring it to another device.

Sending the first message adds **New chat** to the sidebar immediately. The first completed reply supplies its title. A name you choose while the model is working takes precedence.

New conversations use the last **Chat** or **Cowork** mode you selected. Click a work card to expand its steps and check their results.

Use the pencil beside one of your prompts to edit it. **Send** creates another version with the earlier context. Both versions share one sidebar entry. Use the arrows beside the prompt to switch between them.

Use **Import conversations** beside **Chats**, or `/import-chat`. On desktop, select **Codex** or **Claude Code** to browse local sessions and select chats to copy. The file picker also accepts session JSONL files and the ZIP or `conversations.json` from a Claude account export. Spindle copies the visible prompts and replies. It recognizes sessions it has imported before.

Choose **Export conversation…** in the chat's **…** menu or Conversation settings to save a plaintext JSON archive. It includes the selected chat and its saved branches, recorded tool calls and results, approvals, successful and failed steps, model changes, compaction notes, and available timestamps. Request, reply, and action times are UTC Unix seconds. Sync record times are Unix milliseconds. Older records without a saved time leave that field out. Review the export before sharing because it contains the saved conversation and tool data.

Generated PNG and JPEG files preview in the conversation. For several images, select a thumbnail to change the main preview. Paste a URL or write a Markdown link to add a clickable link to a message.

In **Settings → Models → Image generation**, refresh ComfyUI to discover checkpoints and separate diffusion models. The built-in workflows cover SDXL and supported FLUX models. Use **Add workflow** to register a ComfyUI API graph for other nodes. Give it a name and optionally set defaults for named inputs such as `{{steps}}`. The model can use `image_models` to list available choices, then select the model, workflow, and options when it calls `image`.

For direct requests, use `/image --aspect 16:9 --count 2 your prompt`. Set `--size 1280x720` for pixel dimensions, `--transparent` for an alpha PNG, or `--workflow "Workflow name"` to use a registered graph. Built-in alpha output uses a background-removal model installed on ComfyUI. Custom workflows can produce their own alpha output.

Click **…** beside any thread, or right-click it, to expand its options in the sidebar. You can rename, pin, mark unread, move to a project, branch, read, copy, archive, or delete a conversation. **Conversation settings** opens a panel for its title, next model, section, project, and compaction. **Back** or **Close** returns you to what you were viewing. **Branch** starts an independent conversation with the same history. Archived threads are available through **Archive** in the sidebar. Moving a chat changes where its new files are saved. Files already generated stay where they are.

**Delete conversation** asks for confirmation, removes the thread and its queued work and schedules, and propagates the deletion when project sync runs. Earlier encrypted revision files, independent branches, generated files and backups have their own retention. Re-export the phone companion from Projects to use the updated thread list.

The composer shows the latest reported context usage, the active model’s context window when available, and generation speed. Streaming speed uses a visible-text estimate. Final speed uses provider timing when available or reported output tokens divided by elapsed request time. The model answering a new chat supplies its title in the same response. For models that omit a title, Spindle uses the first line of the prompt. Each thread remembers the selected model, and individual responses retain their actual answering model.

Spindle checks the selected model's published output and context limits when available. Ollama and Claude requests start with up to 16,384 output tokens, with room reserved for the prompt in a known shared context. Ollama Cloud uses the hosted context window. Other providers can manage their own output budget. When a model reaches its output limit, Spindle saves the partial reply in the encrypted local queue and requests the missing text. Every continuation passes the same usage and fallback checks. Generated files are saved after the reply finishes. Repeated cutoffs can still pause a long task, and **Resume** continues from the saved text.

Your prompts appear on the right and model replies on the left. For file deliverables, ask for a file such as “Create a Snake game in snake.html”, or name a format (“as a .csv”). Spindle saves the output in the project's **artifacts** folder or the thread's scratch folder in Windows Temp. Each file appears as a card where the reply placed it, with its name, type and size, a primary **Open** (or **Preview** for HTML), and **More** for View source, Edit, Download, Show in folder, Copy path and Save a copy. A file name mentioned in the reply is a link that opens it. Markdown, spreadsheets, Word, RTF and PDF open in the Viewer. HTML opens in the built-in **Browser** tab and uses the isolated preview renderer. Code blocks that stay in the message have Copy and Save file. **/preview <file>** opens a workspace file in the same panel.

On Windows, Spindle sends a system notification when a reply finishes and its window is out of focus. **Settings → Preferences → Windows notifications** lets you hide reply text or turn notifications off. Windows can show notifications on the lock screen.

### Revising work in progress

Use the square **Stop** button beside **Send** to stop a reply or model tool running in this chat. Your draft stays in the composer.

Spindle reads text files in pages sized for the model's context window. It can search a file first and read only the relevant lines, or follow the next page cursor for a sequential read. Automatic compaction waits until the next user request while a tool sequence is in progress.

Switch between **Chat** and **Cowork** in the sidebar. The sync indicator sits beside Settings. Open **Panel → Plan** to edit the current conversation's checklist. Choose **Revise plan…** to describe a change while the model is working. Check **Replace the current objective** when the deliverable itself has changed. For example, `/goal replace Build and test a Windows Rust executable` replaces an HTML goal. Use `/plan revise Include an empty-input test` to keep the objective and amend the plan. Steering an active plan also carries your correction into subsequent tool turns.

In a project chat, ask about a previous discussion or name a distinctive topic from it. For example, “What build flags did we agree on in the previous thread?” Chat and Cowork can draw on short excerpts from other chats in the same project and cite the source chat. This works with Project Memory off and includes project chats already synced to this device.

## Providers and settings

Use **Settings → Model providers → Add provider** to test and save an LM Studio, OpenAI, OpenRouter, Anthropic, or custom Chat Completions connection. Enter the provider's API base URL, including its API version path, and supply a key if required. Spindle fetches the model list. Windows Credential Manager stores keys. Selecting a hosted model sends its request context to that provider. Accounts and billing are managed with the provider.

Model servers accept HTTP and HTTPS addresses. Spindle warns when a remote HTTP connection carries prompts, replies, and any API key in plain text. Use HTTPS when the server supports it.

Add multiple connections of the same type. Each connection has its own address, catalog and optional key. Leave the display name empty to use the suggested name, such as **Ollama-2**, or enter your own.

Click the model control beside the message box or press **Ctrl+K**. Search by model or provider, star favorites, and select a result. Icons beside each model show what it accepts beyond text: an eye for images and sound waves for audio. A brain marks models that can reason. Spindle reads these from Ollama's reported capabilities and provider catalogs, and otherwise from known model families. They describe the model; Spindle does not yet send images or audio.

**Settings → Preferences** controls the accent color (Red, Orange, Yellow, Green, Blue, Indigo, Violet or Magenta), conversation text size, the send shortcut, context compaction and the council roster. The native Windows caption follows the charcoal application theme.

On Android, go to **Settings → Preferences → Updates** and select **Check for updates**. If a newer release is available, choose **Download update**, then **Open APK** to continue in Android’s installer. Android may ask you to allow installs from Spindle the first time.

The sidebar, welcome screen and window icons follow the workspace accent. The Windows executable uses the default blue ribbon. [View all eight icons](../assets/accents/palette.png)

### Choosing which models appear

**Settings → Models** lists every connected model with a search box and a provider filter. Clear a model's checkbox to hide it from the picker. **Show all listed** and **Hide all listed** act on the current search and filter, so you can hide a provider's whole catalog and keep a few. Hidden models stay connected, and work already using them continues.

Click the model name or press **Ctrl+K** to search and choose a model above the composer. The usage rings open compact views of plan usage or the API budget while the chat stays visible.

### Thinking

For models that can reason, click the brain button beside the model picker to choose a thinking level. Spindle shows the levels the model reports or offers Off, Low, Medium and High. **Auto** uses the model's default. The level applies to that model in every chat.

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

### File history

Choose **History** beside a file in **Panel → Files** or **Library**, or choose **More → File history** on a file card. Select a saved version to compare it with the file on disk. Copy the comparison or save a `.diff` file for UTF-8 text up to 1 MiB. **Restore this version** asks for confirmation and saves the current contents first, so you can switch back.

Spindle saves previous contents when its file tools, document editor, or sandbox apply replace files up to 32 MiB. These snapshots stay encrypted on this device. History starts with changes made after the update. Edits made outside Spindle have their own history.

Ask the model to compare file versions or explain what changed. It can call the read-only `file_history` tool within its working folder and request the applied file's history for a project. For revisions to an existing file, ask it to keep editing the same path.

## Review council

Use `/council` to get independent feedback on the latest response, or `/council <question or material>` to focus the review. An approved attachment can supply the material. The council calls its saved reviewers sequentially, with no execution tools, and returns each model's feedback under its own heading. Use `/council roster` to inspect the panel and **Settings → Preferences** to change it. The initial roster prefers local `qwen3.5:9b` and `Maternion/spark-x2.6:4b` when installed.

## Images (ComfyUI)

Spindle tries ComfyUI on this device at port 8188 when the address is blank. For another device, use the host's reachable LAN or VPN address in **Settings › Models › Image generation**, or use `/image server http://model-host.local:8188`. A bare hostname uses port 8188. **Refresh models** lists checkpoints and separate diffusion models and reports any unreadable part of the catalog.

- `/image a red fox in the snow` generates a PNG. Models can also generate pictures with the `image` tool.
- Attach a PNG or JPEG and send `/image edit make the sky violet`, or use `/image edit "red fox.png" -- make the sky violet` with a workspace file or an image created in this chat. The edit saves a separate file.

Both commands have built-in SDXL workflows in ComfyUI. To customize one, use **Workflow › Export (API)** in ComfyUI and set the corresponding workflow path in Spindle. Templates accept `{{prompt}}`, `{{negative}}`, `{{seed}}`, `{{width}}` and `{{height}}`. An edit workflow also needs `{{image}}` as an input value, such as the **Load Image** node's image field. You can also select Stable Diffusion WebUI with its API enabled. That backend uses its currently loaded checkpoint. The selected image server receives prompts and edit source files.

## Audio

Spindle talks and listens through Ollama when you have a model that can (for example `ollama pull lfm3-omni`). Use the microphone button in the message box to dictate, or the speaker button on a reply to hear it. Attach a recording (.wav, .mp3, .m4a, …) and Spindle writes down what is said and attaches the transcript. `/speak some text` saves a WAV in the chat, and models can use `speak` and `transcribe` when you ask. Settings › Models › Audio picks the models and voice, turns on reading every reply aloud, or points Spindle at another server with the OpenAI audio API. You can also use Fish Audio (paid, many voices) for speaking, transcribing or both: choose it there and add your API key.
