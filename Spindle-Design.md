<a id="readme"></a>

# Spindle — design review

The desktop beta is named **Spindle**. The original repository and local session directory keep their existing identifiers so earlier chats remain accessible.

### Desktop beta implementation

The Rust desktop and CLI share a durable workspace queue. Schedules record their chat, prompt, primary model, fallback order, next run and repeat interval. A due run claims its occurrence under the same exclusive lock used by interactive sessions. A headless Windows worker can claim due work when the user enables background scheduling. Missed repeats coalesce into one run. Ordinary resumed work keeps its queue position, steering and approved attachment scope.

Each dispatch checks provider holds and local five-hour and weekly budgets before contacting a model. A budget can request an ordered fallback or a hold until reset. Usage observations retain the provider scope, and the activity log identifies the actual model selected. A timed hold is bound to its task ID. A provider limit without a known reset requires a user-supplied budget reset or manual release. Paired devices exchange encrypted thread revisions through the shared project folder. Each user's credentials, schedules, model routing and budgets live in their private workspace state.

Ollama local inference and Ollama Cloud have distinct destinations for permissions and accounting, even though both use the signed-in local daemon. Maya uses its live model catalog. OpenAI offers an API-key connection and a separate ChatGPT-plan OAuth connection. Claude uses Anthropic's Messages API. The document reader opens files locally and submits excerpts through the workspace's attachment approval flow. [Desktop usage and supported formats](README.md)

### Architecture proposal · 21 November 2026

A lightweight, local-first agent for coding and authorized computer work. Inference can run on a different computer; execution stays native to the computer doing the work.

## Review in two minutes

- **Recommend:** Rust CLI and shared core, with a small native execution broker per platform. Start with an OpenAI-compatible local endpoint; keep model APIs out of policy and tool implementations.
- **First milestone:** a Windows-only, single-agent repair loop on an owned disposable Python repository. Prove containment first, then inspect → minimal patch → test → bounded repair → reviewable diff.
- **Authority:** permissions, revocation, budgets and audit state live outside the model. Local inference receives exactly the same execution controls as hosted inference would.
- **Windows:** AppContainer/LPAC for isolation, explicit file grants and no network capability; Job Objects for descendants and resource limits. Compatibility with the selected Python runtime is an early proof, not a promise.
- **Linux:** mount/user/PID/network namespaces, minimal mounts, seccomp and capability dropping; cgroup v2 for aggregate resources; Landlock where the required rights are supported. Missing mandatory controls disable execution.
- **Android:** native APK plus narrowly brokered isolated workers is the safer direction. Termux remains useful for llama.cpp and a client, but normal Termux/PRoot is not a repository sandbox.
- **Operator workflow:** use `/plan` to inspect and revise steps before execution, `/permissions` to decide pending requests, and `/status` to see what is waiting. Plan acceptance schedules work under the existing grant.
- **UI and /swarm:** separate, opt-in capabilities after the single-agent loop. UI authority never follows from shell/file authority. Swarm workers cannot widen permissions or create more workers.
- **Borrow selectively:** study OpenCode's provider/permission UX and Aider's test-feedback loop; reuse mature platform bindings and Linux sandbox tooling. Build the narrow broker, durable policy state and compact context manager around this project's guarantees.

## Main risks

1. Windows developer tools may require more access than the proposed profile permits.
2. Stock Android cannot promise desktop-equivalent shell containment, resource accounting or UI scoping.
3. Smaller models may produce malformed calls or lose important details; enforce budgets, schemas and invariant checks outside the prompt.
4. Two RX 6800 XTs do not establish usable model size, throughput or multi-GPU scaling. Benchmark the selected runtime/model/driver combination later.
5. UI services often possess broader OS authority than the requested target; broker checks reduce risk but do not become an OS-enforced per-window sandbox.

## Decision and next step

No decision blocks this design. Recommend **M1: Windows containment and one bounded repair**, after a separately authorized implementation phase. Before execution on James's hardware, record Windows build/edition, runtime location and a disposable fixture path. If containment fails, choose a separately reviewed VM profile or leave execution disabled; never add an unrestricted fallback.

All platform behavior below is documentation-backed where cited. No Windows, Android, GPU or sandbox implementation has been tested in this phase.

- [Architecture](#architecture): boundaries, platform profiles, context budgets and commands.
- [Implementation sequence](#implementation): milestones, early proofs and acceptance criteria.
- [Evidence and assumptions](#evidence): primary sources and limits of this review.

No production code, dependency installation, device changes, repository publication or cloud-provider commitment is part of this deliverable.

---

<a id="architecture"></a>

# Spindle architecture

Design proposal · 21 November 2026

**Decision:** a Rust CLI/core, native execution brokers, and a replaceable model adapter. Local inference is the default. This phase specifies behavior; every implementation and performance claim remains untested. Source links support platform primitives, not certification of this proposed system.

## 1. Components and trust

```text
User / optional platform UI
    → session engine + context builder ↔ model adapter ↔ selected inference server
    → typed action proposal
    → execution-host policy broker → file tools / contained process runner
                                  → separately consented UI broker (later)
    → local event journal + evidence store + checkpoints
```

The inference server may be on the Windows workstation, a Linux host, or James's phone running llama.cpp in Termux. The broker, runtime, paths, process controls and UI APIs belong to the **execution host**. An endpoint on another machine changes the data destination, not execution permissions. The first release runs the controller alongside its broker; a phone remote controller is later work.

**Threat model:** model mistakes, prompt injection in source/tool/UI content, hostile repository scripts, malformed calls, accidental scope expansion and interrupted operations. Treat all model output and repository/test content as untrusted. Trust the selected host OS and a minimal broker; host administrators, compromised kernels and compromised brokers are outside the guarantee. Shared-kernel containment does not equal VM isolation.

The broker holds an immutable, versioned grant: execution-host identity, project identity, allowed reads/writes, tool profiles, endpoint/data-transfer scope, UI targets, budgets, expiry and revocation generation. Per-call authority is the intersection of that grant, the tool's requirements and any worker scope. Grants never originate from model text, repository config or summaries. Revocation is checked again immediately before side effects. Approval binds exact normalized arguments, target, content hash and expiry; changed input needs a new approval. Harmless retries inside existing grants do not need repeated user prompts.

Use local ACL-protected IPC with peer validation and short-lived session capabilities. Tool children cannot reach controller, model endpoint, audit storage, credentials or UI IPC. Keep secrets outside workspaces and logs. A later remote control protocol requires explicit pairing, authenticated encryption, replay protection and destination-host policy evaluation; never expose an unauthenticated shell service.

## 2. Model boundary and hardware

**Beta provider selection.** The composer selects a model by provider identity and model ID. Ollama on localhost is the initial destination. Users can connect additional Chat Completions or Anthropic Messages endpoints in Settings. Connection setup fetches the catalog before activation. Hosted endpoints require HTTPS. Windows Credential Manager holds API keys. Queued requests pin their destination, including each swarm or council member. Moving an approved attachment to a different provider requires renewed approval.

### Projects and shared access

A Spindle project has a named folder and a versioned manifest in `.spindle/project.json`. Projects can live on a local disk, a mapped drive, or a UNC share. The Projects view creates, opens and remembers folders for the current user. Opening a second project gives it a separate workspace window. The folder is the shared identity, while each user's local state directory owns their conversations, model settings, skills, queue and approvals.

The local Windows session uses account-bound encryption for messages, queues, attachments and configuration. A legacy plaintext session is read under its existing exclusive lock and replaced atomically with an encrypted copy. Generated files use the selected project’s artifacts directory or a thread-specific temporary directory. Standalone HTML opens in an in-app WebView2 sandbox with inline scripts.

Submitting the first prompt creates a durable **New chat** entry while the model works. After the first reply the thread takes its title from the responding model, with the first prompt line as a fallback. A manual rename takes precedence. Each conversation has its own settings for its title, model, section, pin and archive status. Users can move its history to a project or branch a copy there. Generated files stay in their original folder. A move waits for the user to finish or remove pending work and schedules. Deletion removes the thread from paired devices as they sync. Earlier encrypted revisions can still exist on disk.

Conversation context is assembled for the model selected for each dispatch. Thread settings inherit a workspace compaction threshold, normally 50% of estimated context. Presets at 25% and 75%, a custom percentage and a token ceiling support different cost targets. At dispatch time Spindle extracts earlier decisions, constraints, outcomes and artifact names locally, then packs recent turns. The full transcript and compact checkpoint are encrypted together. Worker and council dispatches keep their existing scope.

For coding requests the harness stores generated files on disk and supplies a short reference to later turns. Successful file writes keep a source snapshot with the encrypted turn. Paste attachments are staged per thread and written when the user sends them. A bounded excerpt of a large attachment can be sent to the selected provider. The desktop places prompts on the right, replies on the left, and interactive HTML or file source in a resizable panel beside the conversation.

Project thread sync is an explicit choice in Projects. Folder permissions and a shared passphrase control access. Each revision uses AES-256-GCM with a fresh nonce, a project-specific key derived with PBKDF2-HMAC-SHA256, and an authenticated revision identifier. The revision contains the thread history, title, answering models, goal, plan and pending-work description. New revisions point to their parent. A receiving device imports completed turns, preserves divergent edits as separate threads, and keeps execution ownership on the originating device. A mobile browser companion can unlock the same files and write an encrypted draft for desktop review. Existing plaintext shares remain readable. Removing someone’s folder access cannot retract copies they already received.

Ollama Cloud account usage is read through the signed-in local daemon. Account windows, reset times and balances have a different scope from the workspace’s per-model dispatch and token counters. The composer reports the active model’s context capacity when the runtime supplies it and labels estimates during streaming.

The adapter translates canonical messages and typed action proposals into provider requests. Its contract includes capabilities/context limits, token counting, streaming generation, cancellation, usage and normalized errors. Capability flags cover tool calls, constrained JSON, images, tokenizer availability and reasoning-token accounting. Probe with harmless fixtures; model listing alone proves none of these.

First transport: local OpenAI-compatible chat completions, with explicit endpoint/model selection, bounded requests and no cloud fallback. llama.cpp documents compatible routes but cautions that compatibility and chat-template support vary. Preserve canonical evidence/messages, not provider-specific opaque summaries, so another local runtime or a later explicitly authorized hosted adapter can take over. [llama.cpp server](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)

Loopback is the same-host default; it is not authentication. For another host use an authenticated encrypted channel and an explicit endpoint allowlist. Do not follow endpoint redirects to unapproved destinations. Repository workers have no network; the controller's model traffic is separate. File/image transmission is part of the endpoint grant. No automatic telemetry, model downloads or provider discovery that sends project data. Later network tasks use a typed broker fetch or a separately tested egress profile, with approved destinations, methods and payload limits. Revalidate redirects and resolved addresses; a broad network capability is not a domain allowlist. Downloaded content remains untrusted and cannot trigger installation.

The dual RX 6800 XT workstation is a candidate inference host, not an execution requirement. llama.cpp documents Vulkan builds on Windows and Linux. Test a single GPU first, then measured split/offload options; do not assume pooled VRAM, linear scaling or ROCm compatibility for these cards. Record model hash/quantization, driver/backend, context/KV memory, first-token latency and decode rate. Phone thermal/memory limits favor one request at a time initially. [llama.cpp build documentation](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md)

## 3. Execution profiles: fail closed

Broker-mediated file operations validate final objects, not just path prefixes: reject traversal, unexpected symlinks/reparse points, hardlink aliasing and changed preconditions. Use handle-relative/no-follow operations where available; Windows path variants, alternate streams and UNC paths require explicit rejection or tested handling. Copy fixtures rather than creating links into real repositories. Config, hooks, test commands and dependencies are untrusted.

Create processes with a clean environment, explicit executable and argv, minimal handles/descriptors and no inherited credentials. A shell command allowlist is not containment; tests execute arbitrary repository code. Disable shell startup files, hooks and package lifecycle/network fetching in the first slice.

| Platform | Native isolation and file/network controls | Process/resource controls | Limits and stronger option |
| --- | --- | --- | --- |
| Windows | Unique AppContainer/LPAC profile; grant only fixture write access and needed runtime read/execute access; no network capabilities. Prefer LPAC if compatible. | Job Object: no breakaway, kill-on-close, process count, memory and CPU limits; deadline watchdog. | Runtime access and ambient OS resources need measurement. Jobs are not file/network security. Optional Windows Sandbox/VM. |
| Linux | Minimal mount tree in user/mount/PID/network namespaces; no host network, sockets or desktop buses; drop capabilities, no-new-privileges, seccomp; Landlock as a checked extra layer. | Delegated cgroup v2 subtree with memory, CPU and pids limits; kill whole cgroup; watchdog and output caps. | Kernel/distro restrictions and cgroup delegation vary. Optional dedicated VM. |
| Android APK | App UID + SELinux, then an isolated service for untrusted work. Broker passes scoped streams/FDs or typed file operations, not general app authority. | OS lifecycle plus watchdog and available per-process limits; no promised desktop-style delegated cgroups. | Initially restricted tools only; general native build/test execution requires device proof. Optional remote execution on an explicitly selected desktop. |
| Android Termux | Android isolates Termux from other apps, not one Termux command from other Termux data. PRoot is path translation, not a security boundary. | Process groups/timeouts are useful supervision, not assured descendant/resource containment. | No general autonomous shell in the default supported profile. Inference/client and bounded broker tools remain possible. |

**Windows details.** AppContainer access uses both user and package/capability authority; LPAC reduces ambient access further. Audit effective ACLs and required integrity labels on the disposable tree; do not grant broad user-profile access. Establish containment before the first instruction runs, using creation-time job assignment or suspended-create/assign/verify/resume; failures terminate the child. Deny unneeded COM/RPC and broker endpoints that could launch outside the job. Test loopback, LAN and internet denial, inherited handles and child/grandchild survival. [AppContainer/LPAC](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer), [Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)

Optional Windows Sandbox uses a separate kernel, has edition/virtualization requirements, and needs its own runtime provisioning. Explicitly disable networking, clipboard and other unnecessary integration; map only controlled exchange directories, read-only where possible. It is a selected alternate profile, never an automatic workaround. [Windows Sandbox](https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/), [configuration](https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/windows-sandbox-configure-using-wsb-file)

**Linux details.** Borrow bubblewrap to assemble namespaces, but own and test the complete policy. Mount only required runtime files read-only, the disposable tree read/write and isolated temporary storage. Do not pass host X11/Wayland, D-Bus, SSH-agent or container-engine sockets. Reject a run if required namespaces or resource controllers cannot be established. [bubblewrap security model](https://github.com/containers/bubblewrap), [cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html)

Seccomp reduces syscall surface; it cannot inspect path strings and is not a complete sandbox. Landlock rights depend on runtime ABI: older kernels have fewer file and networking controls; current documentation includes TCP and newer UDP controls. Never infer support from a distribution label. Close inherited descriptors; apply rules before untrusted work. Default network denial comes from an isolated network namespace, not an assumed universal Landlock address filter. [seccomp](https://docs.kernel.org/userspace-api/seccomp_filter.html), [Landlock](https://docs.kernel.org/userspace-api/landlock.html)

**Android details.** Native and managed code share Android's app boundary; a normal same-UID subprocess is not a narrower sandbox. An isolated service has no permissions of its own. Broker IPC must validate every operation and never pass a broad directory or network capability inadvertently. Use a user-selected Storage Access Framework tree for import/export; keep its URI authority in the trusted broker. [app sandbox](https://source.android.com/docs/security/app-sandbox), [isolated services](https://developer.android.com/guide/topics/manifest/service-element), [SAF](https://developer.android.com/training/data-storage/shared/documents-files)

A modern APK cannot assume it can download and exec toolchains from writable app storage; Android 10 target restrictions constrain that pattern. Package approved native libraries/tools or use an embedded restricted runtime, then prove loading, descendant handling and cancellation on the actual OS/OEM build. No root, ADB, Shizuku, private app data access or SELinux changes are prerequisites. Do not evade restrictions by silently lowering target SDK. Termux's existing toolchain does not establish that an isolated APK worker can reuse it. [Android execution restrictions](https://developer.android.com/about/versions/10/behavior-changes-10), [PRoot limitations](https://github.com/termux/proot-distro#limitations)

All profiles distinguish hard OS limits from best-effort monitoring. Example initial desktop limits: 16 processes, 1 GiB aggregate memory, 50% CPU cap, 120 seconds per test and 256 KiB captured output. These are proposed fixture defaults. Disk usage needs a quota-backed volume for a hard guarantee; polling and output caps are not disk quotas. If a task requires a hard control unavailable on the host, reject it and report the missing capability.

## 4. Optional UI boundary

UI inspection and action are separate grants, both off initially. User selects an app/window/document and duration. Show active capture and a stop control; never covertly record. Keep frames ephemeral by default, redact sensitive content, and log action metadata rather than routine screenshots. Tool/UI text is untrusted evidence, never approval.

Use semantic controls where available; each action revalidates process/package, window identity, document, focus and expected state. Pause on navigation, stale selectors or target change. Preview edits; require explicit approval for sending, purchasing, deleting, credential use, permission changes or other sensitive effects. File authority does not authorize UI, and UI authority does not authorize launching a terminal to bypass file policy.

- **Windows:** standard-user UI Automation for accessible controls; stop at elevated/secure desktops. No UIAccess elevation or UAC bypass. Per-window scoping is a broker promise, not an OS permission that constrains all UI Automation access. [Microsoft UI security](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-securityoverview)
- **Linux:** consented ScreenCast/RemoteDesktop portals where supported; backend capabilities differ. A window capture grant does not imply window-limited input. Defer automation if input target scoping cannot be reliably checked; use a dedicated desktop/VM for stronger separation. No silent X11/global-input fallback. [ScreenCast](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html), [RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)
- **Android:** MediaProjection consent per capture session; prefer app-window capture when available. User-enabled AccessibilityService can expose semantic actions where apps cooperate, but is a powerful separate trust boundary. Respect secure surfaces, password fields and inaccessible controls. A companion APK is needed for this profile; Termux alone does not grant UI authority. [MediaProjection](https://developer.android.com/media/grow/media-projection), [AccessibilityService](https://developer.android.com/guide/topics/ui/accessibility/service), [FLAG_SECURE](https://developer.android.com/reference/android/view/WindowManager.LayoutParams#FLAG_SECURE)

## 5. Small-model context contract

Use the smaller of model-supported context and actual server allocation. All figures are tokens, including chat templates and serialized schemas; these are design allocations, **not measurements**.

| Context | Output reserve | Safety margin | Maximum input | Core pinned | Current goal/constraints pinned | Relevant schemas | Compacted state | Evidence/recent work |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 8,192 | 1,536 | 512 | 6,144 | 768 | 512 | 640 | 768 | 3,456 |
| 16,384 | 2,048 | 1,024 | 13,312 | 1,024 | 768 | 1,024 | 1,536 | 8,960 |
| 32,768 | 4,096 | 2,048 | 26,624 | 1,280 | 1,024 | 1,536 | 2,048 | 20,736 |

Output reserve includes reasoning tokens when the runtime counts them against context. Never drop active constraints to fit. Start compaction near 85% of the input ceiling or sooner if the next bounded result would exceed it. Page results; store full evidence outside the prompt. If fixed content cannot fit, reduce exposed capabilities or stop with a fit error.

Measure rendered prompts with the model's tokenizer/server when available, reconcile reported usage, and record schema/system/evidence tokens, compaction cost, latency and valid-call rate. At 8k the proposed fixed instruction/goal/schema allocation is 1,920 tokens: 23.4% of context and 31.25% of input capacity. Limit advertised tools to about three relevant schemas. Unknown tokenizer counts are estimates: use a conservative UTF-8-byte bound plus measured template overhead until calibrated, shrink chunks and fail rather than truncate constraints. No token-efficiency claims until benchmarked.

Compaction produces a versioned state card: exact active goal/constraints, decisions with rationale/evidence IDs, completed changes and hashes, failing tests, open tasks, rejected approaches, unresolved outcomes, next action and original evidence references. Trusted user constraints are rehydrated from the ledger verbatim; model-generated summaries cannot rewrite them. Validate required fields and evidence IDs against the ledger, including completeness of active decision IDs, pending tasks and unresolved outcomes. On failure use a deterministic ledger extract and selected recent evidence. Keep original events locally so omitted details can be retrieved. Switching models rebuilds the prompt and recalculates limits while retaining the same broker grant and revocations.

Quarantine tool/source/UI content in typed evidence records with provenance, hashes and explicit untrusted labels; never splice it into system instructions or execute embedded commands. These labels help the model but are not a security boundary.

For models without native tools, accept one compact JSON envelope, e.g. `{"v":1,"id":"c7","tool":"read","args":{"path":"src/calc.py","start":1,"lines":60}}`. Validate against only the current tool schema; reject extra fields, unknown tools, mixed prose and truncated output. Allow one format-repair attempt, then stop. Never parse prose/code fences as shell. Use constrained decoding if supported, but enforce authorization identically in both paths.

## 6. Loop, commands and recovery

Single-agent state machine: **inspect → plan a minimal change → apply conditional patch → run approved tests → evaluate → revise or finish**. Default ceiling: 12 dispatched tool actions, three repair cycles, 15 minutes, plus a configured cumulative token budget. Stop on repeated identical failure twice, no progress, exhausted budgets or a new permission need. A test failure is feedback inside the goal, not a request for another human prompt. Tests remain authoritative only for the tested behavior; protect acceptance fixtures from model edits.

Registry entries have name/version, argument schema, help, local deterministic handler, required capabilities and lifecycle hooks. Commands are parsed from authenticated user input, never tool output. No arbitrary plugin code in phase one; later extensions use explicit installation and bounded capabilities.

| Command | Concrete semantics |
| --- | --- |
| `/goal <text>` | Persist a goal with success criteria and scope; propose missing criteria. Run only within existing grants. `/goal show` inspects it; `/goal replace <text>` checkpoints and replaces it at a safe boundary. Neither expands permissions. |
| `/plan [show\|revise <text>\|accept <revision>]` | With an active goal, enter planning at a safe boundary and draft the steps, dependencies and verification criteria. Show displays the current revision. Revise records an amended draft. Accept schedules the named revision under the current grant. |
| `/swarm <task>` | M5 delegation command for one or two non-recursive workers. Repeat `--worker <role>=<model-profile>` to choose each worker's role and model. The existing `--workers 2` form uses the coordinator's model for both. Preview assignments and budgets before dispatch. |
| `/model [profile]` | List configured model profiles and their endpoints, or switch the coordinator at a checkpoint. Running workers keep their assigned profiles. |
| `/queue [add <text>]` | Show waiting follow-ups in order or append one. `/queue remove <id>` removes a waiting item. `/queue steer <id>` redirects a waiting item into the active run. |
| `/steer [--worker <id>] <text>` | Give the active coordinator a correction or new constraint. At M5 the user can target a worker by assignment ID. Show whether the instruction is pending or applied. |
| `/skills [list]` | List installed skills and their commands. Author with `/skills create <name>`, `/skills validate <directory>` and `/skills test <directory> <task>`. Manage installed versions with `install <source>`, `show <name>`, `update <name>` and `remove <name>` under the same command. |
| `/status` | Show goal, plan revision, current and blocked steps, pending request IDs, diff/test state, remaining budgets, endpoint and effective capability profile. In swarm runs include worker IDs and outcomes. |
| `/cancel` | Revoke dispatch immediately, cancel generation, terminate contained processes, checkpoint and report uncertain effects. Also available as Ctrl-C. |
| `/resume <session>` | Reconcile journal, verify host/profile/grants and file hashes, then continue unfinished work. Never replay uncertain mutations. |
| `/diff` | Show changes against the recorded baseline; no commit or publication. |
| `/compact` | Run validated compaction at the next safe boundary; permissions remain external. |
| `/permissions` | Show grants/denials; trusted UI can revoke or approve exact new scope. Model cannot approve. |
| `/help [command]` | Show registry-backed usage and capability requirements. |
| `/council [review question]` | Built-in read-only review skill. Send the submitted material and latest completed answer to a saved panel of one to four distinct provider/model selections. Each reviewer receives an independent context. Preserve labeled feedback from each member in the conversation. |
| `/council roster`, `/council set <model> ...` | Inspect or replace the workspace roster. Settings exposes the same selections. A running council retains its assigned models. An unavailable member holds the review and preserves completed feedback for resume. |

### Queueing follow-ups and steering active work

While the agent is busy, submitting an ordinary message queues a follow-up. The CLI acknowledges it immediately with its position and ID and offers **Steer now**. For example, queue “Write release notes after the fix” and steer with “Keep the public signature unchanged.” The notes wait for their turn. The signature constraint applies to the current fix. Authenticated control commands such as `/cancel`, `/permissions`, and `/status` remain responsive during generation and tool calls.

**Queue.** Store each submission under a client request ID and session sequence before acknowledging it. Start waiting items in arrival order, one coordinator turn at a time, after the active turn finishes successfully. An idle session can start a queued item immediately. Each item starts with the then-current workspace, grants and budget checks. A blocked run, cancellation, failure or uncertain effect holds the queue. Show the reason and waiting items in `/status`. On `/resume <session>`, reconcile the interrupted run before releasing waiting work. Promotion, removal and scheduler claims compete atomically for the same waiting item. If the scheduler already claimed it, report the active turn ID so the user can steer it directly.

**Steering.** Bind the message to the current run and optional worker ID. Record its sequence, suspend new dispatch for the target, interrupt unfinished model generation, and discard partial tool calls. If a tool is already executing, record its observed outcome before replanning. The CLI shows “waiting for current action” until the correction is incorporated. Apply pending instructions in order, retain them through compaction, invalidate superseded proposals and approval requests, then recheck the accepted plan and permissions. A scope change follows plan review. `/cancel` remains the immediate stop control. If the target finishes first, report the steering message as unapplied and offer to queue it. On restart, reconcile applied sequences before resuming dispatch.

A coordinator correction that affects delegated work pauses dispatch for the affected branches. Each live worker receives the relevant instruction at its next safe boundary. The coordinator rechecks already returned diffs against the correction before integration. Steering a specific worker preserves its assigned model, budget and grant.

### Building, installing and using skills

**Authoring example.** Run `/skills create release-notes` and describe the intended workflow, the inputs it needs, and an example of a good result. The harness proposes a project-local draft at `skills/release-notes/SKILL.md`, with optional templates or examples. It previews the files and writes through the normal patch broker. If the directory already exists, offer to revise its existing files with a reviewable diff. A user can also ask the builder to turn selected steps from a completed run into a reusable skill. Preview those steps and replace run-specific paths, private evidence and credentials with input parameters before saving.

Iterate on the draft using ordinary edits or steering. `/skills validate ./skills/release-notes` runs the installer's package checks and reports errors by file. `/skills test ./skills/release-notes "Summarize the fixture changes"` loads the draft into a disposable fixture run and reports the selected model, outputs, requested actions and checks against the example result. External side effects use fixture substitutes during this test. After review, install the tested directory through the standard installer. Link the test result to the draft digest so subsequent edits visibly mark that result as stale.

A skill is a directory containing `SKILL.md` with a name, description and instructions, plus optional referenced resources. The first installer accepts a local directory or an explicitly selected Git repository, revision and optional subdirectory. For example, `/skills install ./skills/release-notes` previews a local skill. Remote installation resolves the revision to a commit and uses the controller's authorized fetch path. Show the source, resolved revision or local content digest, command name, included files and any declared tool needs before applying the installation.

Store validated files outside the worker tree in a user-managed registry, enabled for the selected project by default. Reject path escapes, links outside the package, malformed metadata and command collisions. Stage and validate the complete package before atomically activating it. Installation runs no package scripts. Once installed, `/release-notes <task>` invokes the example skill through the command registry. `/help` and `/skills show release-notes` identify its source and active version. Load its instructions on invocation and keep referenced resources within the worker's context and data-transfer limits. Skill instructions carry no grant authority. Any requested tool action uses the normal broker and approval flow.

Update previews the version and file changes and swaps versions only after successful validation and user acceptance. An active invocation stays pinned to its recorded digest. Removal disables new invocations immediately and preserves the audit record for existing runs. A queued invocation records the chosen digest at submission. If its skill was removed or replaced before dispatch, hold the item and show the change for review.

### Choosing worker models

Use one command for individual subagents and mixed-model swarms. For example, with locally configured profiles named `local-code` and `local-reason`:

```text
/swarm "Review the pending diff" --worker reviewer=local-reason
/swarm "Fix the parser and identify missing edge cases" --worker implementer=local-code --worker analyst=local-reason
```

Each explicit worker entry creates one assignment. Role labels identify the brief in the preview and results. The coordinator sets file scope, permitted actions and a completion condition separately for each assignment. With no worker options, default to one worker using the coordinator's profile. Count-only requests support one or two workers. If a count and explicit entries are supplied together, their counts must agree. Reject duplicate role labels, unknown profiles and excess workers before starting any assignment.

Resolve each profile to a model ID, adapter and endpoint. Preview the role-to-model mapping, destination, evidence scope and per-worker budget. Check transfer authority separately for each destination before sending project data. Build each prompt against its own context window and tool-call format. Record the resolved profiles with the assignment IDs and expose them in `/status` and result evidence. If a selected model becomes unavailable, show the affected assignment as blocked and preserve completed sibling results. Changing its model requires an explicit selection and a fresh destination check.

### Other command candidates

These are candidates for the registry after the core loop. Existing `/goal`, `/compact`, `/diff` and `/status` remain the entry points for their current operations.

| Commands | Proposed behavior |
| --- | --- |
| `/review [target]`, `/init` | Review the working diff by default or a named local commit or branch. Offer a review worker with model selection at M5. Init previews a project instructions file and preserves existing instructions when applying approved edits. |
| `/new`, `/fork [checkpoint]`, `/archive` | Start a fresh session, branch a conversation from a checkpoint, or hide a finished session from the active list. Show the workspace attached to a fork. Creating a separate worktree is an explicit option. |
| `/undo` | Preview the last reversible change and its recorded hashes. Restore only matching harness edits. Show any concurrent user edits or external effects that need separate handling. |
| `/open`, `/add <file>`, `/usage` | Search available files, sessions and commands, attach a scoped file reference, or inspect token and time consumption by model and worker. Check destination scope before sending attached content to a model. |
| `/mcp`, `/terminal`, `/fast` | Surface these only when the corresponding capability exists. Show configured MCP server status, open a supported contained terminal, or select a supported speed profile with its usage implications. |
| `/report`, `/feedback`, `/share` | Preview the redacted diagnostics, feedback or session excerpt locally. Any external delivery needs a chosen destination and an explicit send decision. |

Document, slide, design, dashboard and research shortcuts can use the skill registry. The palette identifies each installed skill and its source, shows declared capabilities, and reserves built-in command names.

**Planning.** The ordinary repair loop can still plan and test a small change within its existing grant. Explicit `/plan` mode checkpoints execution and permits scoped reads, reasoning, and controller writes to the private plan journal. The broker disables project writes, test execution, UI actions and worker dispatch in this mode. Each step has a stable ID and an observable completion condition. Goal replacement invalidates the old plan. Accept checks goal ID and revision atomically. A stale acceptance shows the current draft and starts no work. If the accepted scope needs to change, checkpoint affected steps for review. A failed test inside the accepted scope can proceed through the existing repair loop.

**Approval requests.** At dispatch the broker returns **allow**, **ask**, or **deny**. Allow covers an action within the effective grant. Ask applies when a supported action needs user consent. Deny covers hard profile restrictions, revocation, and actions unavailable to the caller. For example, a Windows test profile that forbids network access must deny a network operation even if the model asks for a prompt.

For ask, journal a pending request and suspend the action. The trusted CLI lists it under `/permissions`, including requesting worker or coordinator, plan step if present, execution host, target, operation, expected effect, expiry and reason. Show a patch preview or the broker-defined test profile and argv as applicable. Offer **allow once** and **deny**. For repeatable work the user may explicitly grant a displayed bounded scope for the session, subject to the same profile ceiling and revocation checks. Sensitive UI effects still require exact approval. Closing the prompt leaves the action pending until expiry or cancellation.

Before dispatch, revalidate the request against the live grant generation, arguments, target and content hash. Atomically claim its invocation and consume any one-time approval. A second client sees the recorded decision. A changed action needs a fresh decision. Cancellation invalidates pending requests and stops descendants. Resume restores only valid pending requests and reconciles claimed invocations before proceeding. Plan acceptance grants no additional capabilities.

Journal every intent before dispatch and every result afterward: call ID, normalized arguments/hash, permission decision/grant version, model/template identity, budget use, exit status, affected-file hashes and evidence IDs. Redact secrets. If the intent cannot be durably logged, do not dispatch the side effect. Store an append-oriented journal and content-addressed evidence outside the worker tree; checkpoints are derived, not a second source of authority. A hash chain detects accidental changes, not tampering by a host administrator.

Retry transient read/model transport errors at most twice with bounded backoff and jitter. A dropped stream cannot dispatch a partial action. Mutation IDs are deduplicated; after a timeout/crash inspect recorded status and actual file state before retrying. Patches require expected hashes and atomic replacement where supported; multi-file operations have per-file recovery records. Do not promise exactly-once arbitrary shell effects.

On cancellation or crash, mark running actions interrupted, kill/reap the profile's descendants and preserve evidence. Resume requires reconciliation; UI actions with uncertain results pause for inspection. Rollback affects only known harness changes with matching hashes; never discard concurrent user edits. Lock a project against concurrent coordinators.

---

<a id="implementation"></a>

# Spindle implementation sequence

Design only · proposed gates, not work performed · 21 November 2026

## Recommendation and packaging

Choose **Rust CLI + Rust core + native platform brokers**. Keep optional Kotlin Android UI and Windows/Linux UI shells separate. Start with per-platform binaries and a local data directory; defer installers, auto-update and GUI frameworks until behavior is proven.

| Option | Practical fit | Cost / decision |
| --- | --- | --- |
| Rust | Windows/Linux native targets and Android library targets; Microsoft-maintained Windows bindings. Good fit for a small explicit broker and shared protocol/state code. | FFI and OS security code need focused review; Android still needs platform glue. **Recommend.** [Rust targets](https://doc.rust-lang.org/rustc/platform-support.html), [windows-rs](https://github.com/microsoft/windows-rs) |
| C#/.NET | Strong Windows integration; self-contained or Native AOT deployment, with Linux support. | Native AOT restricts dynamic loading/code generation and built-in COM; UI Automation integration needs deliberate interop choices. Android packaging is a separate decision, not a single portable console binary. Reasonable alternative if Windows delivery speed dominates. [.NET Native AOT](https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/) |
| TypeScript + runtime | Fast provider/TUI iteration; can borrow ideas from the OpenCode codebase. | Security-critical native sandbox helpers remain necessary; runtime/package surface and Android support need proof. Prefer for an optional client rather than the initial trusted broker. [OpenCode repository](https://github.com/anomalyco/opencode) |

Do not assume language choice creates isolation. Keep unsafe/native calls small, reviewable and behind versioned interfaces. Build no dynamic extension loader in M1. An eventual GUI can be platform-specific; Tauri is an option, but its system webview/build dependencies are another packaging layer, not required by the CLI. [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

## Build or borrow

OpenCode's Plan role provides a precedent for restricting edits during planning. Use the command registry for discoverability and enforce mode restrictions in the execution broker. Hermes persists a delegated job's completion before notifying the parent. Apply that ordering to swarm results so a coordinator restart cannot lose a completed assignment. [OpenCode agents](https://opencode.ai/docs/agents/) · [Hermes delegation](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/delegation.md)

- **OpenCode:** current primary docs show configurable providers, resource permission rules and summary compaction. The repository is MIT-licensed. Use it as a baseline for UX/provider compatibility and possibly a later client. Its documented permission rules do not establish this project's Windows/Android OS containment guarantees. Inspect a pinned revision before borrowing code; no claim that OpenCode has been comprehensively security-audited here. Its documented default 15k recent-token retention is unsuitable unchanged for our 8k profile. [providers](https://opencode.ai/docs/providers/), [permissions](https://opencode.ai/docs/permissions/), [compaction](https://opencode.ai/docs/compaction), [license/repository](https://github.com/anomalyco/opencode)
- **Aider:** Apache-2.0 repository; documented auto-test and repair behavior is a practical reference for a bounded coding loop. Borrow the workflow and evaluate patch/repository-context techniques; do not treat its terminal workflow as our sandbox. [repository](https://github.com/Aider-AI/aider), [testing](https://aider.chat/docs/usage/lint-test.html)
- **Infrastructure:** reuse llama.cpp as an external inference server, windows-rs for Win32 bindings and bubblewrap on Linux. Own the capability broker, state journal, context budgets and platform conformance tests. Pin dependencies, inspect notices and review transitive licenses before copying/shipping.

A thin new core is justified by the combination of native execution, explicit Android limits and small-context invariants. Revisit borrowing more after M1: if an existing harness can route every effect through the same broker and meet the same acceptance suite, prefer integration over duplicating its UI.

## M0 — specify contracts and prove the hard assumptions

**Deliverables in a future implementation phase:** versioned model/action/event/grant schemas, threat-model review, platform capability probe specification and owned fixture definitions. No device probing was performed during this design.

Run these proofs early, even when the corresponding product adapter comes later:

1. **Windows:** chosen standard-user Windows build; AppContainer/LPAC creation, runtime DLL/import access, disposable-tree ACL/integrity labels, job membership before execution, no network and no out-of-job descendants.
2. **Linux:** available unprivileged namespaces, seccomp, Landlock ABI and actual cgroup delegation/controllers. Confirm no ambient desktop/agent sockets.
3. **Android:** OS/API/OEM and Termux build recorded; APK isolated-service IPC/FD constraints, packaged runtime feasibility, lifecycle death and cancellation. Confirm no assumptions about root/user namespaces/cgroups. Test existing llama.cpp endpoint separately from execution permissions.
4. **UI:** Windows standard-user target access; Linux portal/backend capture/input scope; Android consent and accessible controls in an owned app. Determine which actions remain manual.
5. **Inference:** selected local server context allocation, tokenizer/tool templates, malformed-call behavior and cancellation. Benchmark one RX 6800 XT before multi-GPU; phone separately. No GPU configuration or downloads without the later implementation scope.

**Gate:** each mandatory capability is supported-and-tested or explicitly unavailable. Documentation alone cannot mark a profile ready. Missing controls yield a capability report and zero process dispatches.

## M1 — first useful Windows slice

Use a fresh owned repository copy with a small Python standard-library function and unit tests, no dependencies, secrets, hooks or external services. Choose an already provisioned, explicitly scoped Python runtime; prove it works in the profile first. Keep immutable acceptance tests outside model-writable paths.

Implement in dependency order:

1. Standard-user CLI, local session/event storage and read-only status.
2. Broker grants, pending approvals in `/permissions`, and safe file reads/search/conditional patch operations.
3. Windows containment launcher, capability checks, resource limits and cancellation.
4. One local OpenAI-compatible adapter; native-tool and compact-JSON paths.
5. Context accounting, durable state card and revisioned `/plan`, then the bounded inspect/change/test/repair loop.
6. Reviewable diff and verification report; no automatic commit, push, install or publication.
7. Durable follow-up queue and steering through the CLI event loop. Add skill drafting, validation, fixture testing and the local installer once the dispatcher and permission checks are in place. Enable Git sources when the scoped controller fetch path is available.

**Demo goal:** “Correct this function's boundary handling; preserve the public signature and make the protected tests pass.” Use a fixture that causes the first model patch to fail, then require repair from feedback without another prompt. Test commands are broker-defined argv profiles, not model-provided shell strings. Launch policy, not the repository's test script, determines containment.

**Gate:** tests A1–A6 and A9–A13 below pass for the implemented profile; A7 proves UI absent/denied. Run the remote-source case of A13 when enabling Git installs and the worker case of A12 at M5. Record failures as failures. No broadening file/network access merely to make Python work.

If native containment proves incompatible, stop execution work. A separate Windows Sandbox/VM profile can be proposed with its own exchange directory and tests; it is not enabled automatically. The working CLI may still inspect approved files and propose diffs.

## M2 — Linux parity and measured context reliability

Port the broker contract using minimal bubblewrap policy, seccomp, capability dropping, checked Landlock and delegated cgroup v2. Reuse owned fixtures and all transport/recovery tests. Run 8k, 16k and 32k context pressure tests; publish prompt-category tokens, completion rate, format-repair rate, compaction cost and latency with exact model/runtime versions. Do not optimize away controls.

## M3 — Android restricted mode

Package a Kotlin APK shell with shared Rust logic where useful. Start with consented import/edit/export and typed broker tools; try an isolated worker with an embedded approved interpreter/runtime. General build/test tooling remains disabled until packaged-code, file/FD, network and cancellation proofs pass on named device versions.

DayTasks established an on-device Termux build for a small Java/XML app using `javac`, D8, `aapt2`, `zipalign`, and `apksigner`. For Spindle, bundle Android-compatible compiler components and call them directly. The [Java build probe](android/builder/README.md) tests an in-memory ECJ and D8 pipeline in an isolated Android service without Termux. Release builds need redistributable framework stubs, manifest/resource linking, per-project debug signing, and the isolation checks above. Gradle support is a separate target because its build scripts and plugins execute code.

Use James's Termux llama.cpp as an inference endpoint if separately authorized. A Termux CLI client may control an explicitly paired desktop broker; it must label the execution host. No implicit offloading when “run on phone” was requested. Remote desktop execution does not count as native phone execution.

## M4 — optional authorized UI

Build adapters only for tested platform/target combinations. Separate inspection/action grants, visible capture, target validation, previews and stop/revocation. Run A7 with an owned editor/app and synthetic sensitive dialogs. Enable neither workers nor arbitrary desktop capture by default.

## M5 — bounded /swarm

Resolve the full worker roster before scheduling. Reserve budgets across models against the parent's remaining total. Different model assignments may run sequentially when hardware can host only one at a time. Persist each resolved model and endpoint so that resume can verify the original selection even if a profile alias has since changed.

Only after stable single-agent runs. Two non-recursive workers, disjoint scopes/copies, small task packets and explicit total budgets; serialized inference initially on small hardware. Workers return diffs/evidence; coordinator integrates and retests. Enforce parent grant intersection and aggregate budgets in broker state, not worker prompts. Conflict or unresolved outcome stops integration. Cancellation revokes every worker.

Persist a worker ID, parent step, brief, evidence references, writable scope, reserved budget and completion condition before dispatch. Start each worker with a fresh context. Queue ready assignments when both slots are occupied. An out-of-scope dependency returns to the coordinator for reassignment or user review. Persist the terminal result, changed paths, check outcomes and remaining issues before notifying the coordinator. On reconnect, consume results by worker ID and integrate each accepted diff once. Return unused budget when the worker exits. M5 must pass A8, including the restart case.

## Acceptance suite

All fixtures are owned local data/apps and local controlled listeners. No scanning, third-party targets, real accounts or sensitive personal data. Each result records OS/runtime/model version, profile/grant hash, evidence IDs and observed outcome. Tests below are specifications, not passed results.

| ID | Setup and required result |
| --- | --- |
| **A1 Model interchange** | Run the same task through two local model/runtime configurations, including one without native tool calls. Switch at a checkpoint to a smaller context. Canonical state, file scope and permission hash stay unchanged; unknown schema fields/capabilities fail. Assert no unapproved endpoint receives bytes, and model-server traffic never comes from a worker. |
| **A2 Feedback repair** | First patch fails a protected edge-case test. Agent reads bounded test output and fixes it within three repair cycles without another human prompt. Protected tests/signature remain unchanged; final suite passes. An impossible fixture stops at the budget with evidence instead of claiming success. |
| **A3 Compaction pressure** | At each 8k/16k/32k limit, inject large logs and enough turns for three compactions. Preserve exact goal/constraints, active decisions, unresolved action ID and evidence links. Deliberately omit a decision in a summary: validator/deterministic fallback retains it. Token ceiling and output reserve hold; malicious evidence cannot grant access. |
| **A4 Fail-closed isolation** | Missing/failed mandatory sandbox step creates no uncontained process. From a contained owned fixture attempt outside-tree read/write, symlink/reparse traversal, inherited-handle access, loopback/LAN/controlled-network contact and child/grandchild escape. Expect denial while required fixture/runtime operations work. Exercise missing Linux controllers and unavailable Android isolation without permissive fallback. |
| **A5 Cancellation/resources** | Bound CPU/memory/process-fanout fixtures to safe quotas; verify configured OS limits and evidence of any best-effort controls. Cancel: dispatch stops immediately and owned descendants stop within a proposed two-second hard-kill deadline after any configured grace period. Broker crash also terminates the job/profile or yields a documented failed acceptance result. No survivor continues writing. |
| **A6 Recovery/flaky tools** | Drop a response after a patch commits; restart mid-tool and with a concurrent user edit. Reconcile by call ID/hash, never double-apply or overwrite that edit. Interrupt a model tool-call stream: zero partial calls execute. Repeated read failures stop after two retries; uncertain shell/UI outcomes pause. |
| **A7 UI isolation** | Before UI grant, file/shell sessions cannot capture or drive UI. After scoped consent, inspect/edit only the owned target; target switch, expiry, revocation and stale state stop actions. Synthetic send/delete requires exact approval; secure/elevated/inaccessible UI stays blocked. No frames retained by default, no UI IPC available to test processes. Later platform adapter must pass again. |
| **A8 Swarm scope** | Worker tries a sibling path, forbidden endpoint, UI action, recursive spawn and budget overrun: broker denies each. Conflicting patches are not silently merged. Parent cancellation stops both workers; combined spend never exceeds parent budget. |
| **A8 restart case** | Restart the coordinator after a worker records its result and before notification. Deliver the recorded result twice. The coordinator accounts for the worker once, integrates the patch once and records the combined test result. |
| **A8 model routing** | Give two workers different local endpoints, context limits and tool formats. Verify each endpoint receives only its assigned brief and authorized evidence, and each result identifies its model. Repeat with one explicit worker, then with the original count-only form. Reject an unknown profile before dispatch. Interrupt one endpoint mid-run and verify the sibling result survives with no silent model substitution. |
| **A9 Plan lifecycle** | Draft a plan and request a patch, test run and swarm dispatch while planning. Each is blocked. Revise the plan and accept the stale revision. Zero steps start. Accept the current revision and verify that the first ready step uses the existing grant. Replace the goal and verify that the old plan cannot start. |
| **A10 Approval lifecycle** | Pause an eligible action, inspect its request in the CLI, then deny it and verify zero effects. Repeat with approval from two clients and observe one claim. Change the target after consent, expire the request, revoke its grant and cancel the run in separate cases. Each prevents dispatch. Restart with a pending request and confirm the decision is still required. A hard-denied action never offers approval. |
| **A11 Queue** | Queue two messages during a test, resend one submission ID and restart. Check FIFO order, one turn per item and a responsive status command. Race promotion against dequeue and verify one delivery. Cancel with an item waiting and verify it remains held until resume. |
| **A12 Steering** | Steer during generation and during a running tool. No discarded partial call executes. The next dispatch incorporates the correction and rejects superseded approvals. Restart after acknowledgement and check the correction is applied once. At M5 target one worker, then steer the coordinator and verify all affected diffs are rechecked. |
| **A13 Skills** | Install a local fixture, find its command in help and invoke it. Verify a permission-sensitive step still asks. Reject a colliding command and an escaping resource path. Interrupt an update and verify the old version works. Remove the skill with an invocation queued and verify the item is held. For Git installs verify the recorded commit and zero activation on fetch failure. |
| **A13 authoring case** | Create a draft from a sample workflow. Confirm the command stays out of the palette until installed. Introduce a missing resource reference, validate, correct the reported file and run the fixture. Edit the instructions and verify the previous test is marked stale. Retest, install and verify the active digest matches the tested draft. |

A4 cannot establish immunity from kernel vulnerabilities or every possible escape; it is a regression gate for the claimed profile. Peer review of native boundary code remains necessary before expanding beyond owned disposable fixtures.

## Blocking decisions and stop conditions

**No blocker for the completed design.** The recommended first implementation milestone is M1, with M0 proofs as its entry gate. James need not select a cloud provider.

Before hardware work, record the chosen Windows OS/runtime and confirm an owned disposable location in that new scope. The actual go/no-go is whether that combination passes containment. If not, the decision is between a separately reviewed stronger isolation profile and retaining non-executing tools.

General Android shell execution is a capability risk, not a promised later feature. UI support is target-specific. Neither uncertainty should block the Windows single-agent design.

---

<a id="evidence"></a>

# Evidence and assumptions

Reviewed 21 November 2026. Primary documentation was consulted on the web; no source checkout, runtime installation, OS sandbox probe or GPU benchmark was performed. Links point to living documentation. Pin source revisions and OS/runtime versions when implementation begins.

## Status vocabulary

- **Documented:** a platform/project explicitly describes the primitive or behavior.
- **Proposed:** Spindle policy or architecture choice.
- **Untested:** whether that choice works on James's particular hardware, OS and tools.
- **Unavailable:** a required capability cannot be established; execution stays disabled.

All numerical budgets, default quotas, cancellation deadlines and acceptance thresholds are proposed.

## Source-to-claim map

| Primary documentation | What it establishes | What remains untested |
| --- | --- | --- |
| [AppContainer/LPAC](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer) | Capability/token/ACL-based Windows isolation and narrower LPAC access. | Python/toolchain compatibility and complete effective resource grants. |
| [Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects) | Process grouping, resource controls and termination; documented creation/breakaway caveats. | No surviving or broker-launched descendants in our profile. |
| [Windows Sandbox](https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/) and [configuration](https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/windows-sandbox-configure-using-wsb-file) | Separate-kernel option, edition requirements and configurable integration/networking. | Hardware availability and controlled import/export workflow. |
| [bubblewrap](https://github.com/containers/bubblewrap), [seccomp](https://docs.kernel.org/userspace-api/seccomp_filter.html), [Landlock](https://docs.kernel.org/userspace-api/landlock.html), [cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html) | Complementary filesystem/namespace/syscall/resource primitives; policy and ABI matter. | Selected distro configuration, delegation and complete profile composition. |
| [Android app sandbox](https://source.android.com/docs/security/app-sandbox), [service manifest](https://developer.android.com/guide/topics/manifest/service-element), [SAF](https://developer.android.com/training/data-storage/shared/documents-files) | App UID boundary, permissionless isolated services and user-selected document access. | Runtime/FD broker design and OEM behavior. |
| [Android 10 changes](https://developer.android.com/about/versions/10/behavior-changes-10), [PRoot](https://github.com/termux/proot-distro#limitations) | Writable-home execution restrictions for relevant APK targets; PRoot does not supply kernel namespaces/cgroups. | Safe general native coding execution on the phone. |
| [Windows UI security](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-securityoverview), [Linux ScreenCast](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html), [RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html) | UI privilege boundaries and portal session/source/device contracts. | App-specific semantics and reliable target-limited actions. |
| [Android projection](https://developer.android.com/media/grow/media-projection), [accessibility](https://developer.android.com/guide/topics/ui/accessibility/service), [secure flag](https://developer.android.com/reference/android/view/WindowManager.LayoutParams#FLAG_SECURE) | Capture consent, accessibility service facilities and protected surfaces. | Consent/revocation behavior and edit support on selected apps/device. |
| [llama.cpp server](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md), [build](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md) | Compatible API routes, template caveats and Vulkan build paths. | Actual model/tool reliability, dual-GPU scaling, memory and phone performance. |
| [OpenCode](https://github.com/anomalyco/opencode), [providers](https://opencode.ai/docs/providers/), [permissions](https://opencode.ai/docs/permissions/), [compaction](https://opencode.ai/docs/compaction) | Provider configuration, application permission model and context strategies; MIT repository license. | Suitability of any pinned component for this broker/security model. |
| [OpenCode agents](https://opencode.ai/docs/agents/) | Role-specific restrictions during planning. | Enforcement of Spindle plan mode through every broker entry point (A9). |
| [Hermes delegation](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/delegation.md) | Background delegation and durable completion before notification. | Recovery and deduplicated integration on this coordinator (A8). |
| [Aider](https://github.com/Aider-AI/aider), [test loop](https://aider.chat/docs/usage/lint-test.html) | Apache-2.0 repository license and automatic test-feedback repair. | Small-model success rate and compatibility with our bounded loop. |

## Deliberate limits

The presence of an API is not proof of an end-to-end security boundary. Permission labels, model instructions, path globs, process groups, PRoot and ordinary resource monitors are not substitutes for enforcement.

UI grant scoping may depend on trusted broker checks because underlying APIs can be broader. Native APK sandboxing does not grant a desktop toolchain. Local model hosting does not reduce the authority of executed code or prevent a model server from seeing submitted data.

No hidden fallback, dependency download, elevated helper, root requirement or provider enrollment is approved by this design. Any future implementation must report its actual effective profile and unsupported capabilities.
