# Bobbins

A Bobbin is a named Spindle worker with its own model, purpose, instructions, appearance, assignments, and run history. Open **Bobbins** in the sidebar or Android drawer. The `/bobbins` command opens the same view.

## Start a run

1. Choose **New bobbin**, then enter a name and purpose or use a starter profile.
2. Pick a connected model. Select a project if the run chats should appear with its conversations. Enable project reads, run output files, or public web access individually.
3. Choose **Save draft**. Open the Bobbin and write a message, or create a reusable assignment with completion criteria. Choose **Run now** to submit it.
4. Open **Runs** to see the model, home, timestamps, latest status, and result chat. Use **Steer** while the run is in the queue. **Cancel run** stops the selected run and keeps work already recorded.

Profiles start with prompt-and-reply access. A profile can read one selected project, write deliverables in its own run chat, and search or read public pages when you turn on those options. Project folders stay read-only for Bobbins. Each run retains its selected model and assignment criteria. Changes to scope take effect on later tool actions. Existing Spindle workspace controls still apply.

**Pause after current runs** holds queued work and stops new dispatch. **Pause and stop runs** cancels current work too. **Resume** returns held work to the queue. Archive disables schedules and keeps the history. Duplicate creates a new draft with the same appearance and instructions. The copy starts with fresh access settings and schedules.

## Schedule work

On Windows or Linux, open **Assignments → Schedule** and preview the first run. Choose a one-time start or a fixed repeat interval in elapsed hours or days. **Enable schedule** authorizes future runs. **Run now** is separate. Spindle has to be running in the desktop app or its existing background runner when work is admitted. Overdue repeating runs combine into one run unless you select **Skip missed runs**. Changing instructions, model, or access pauses schedules for review.

## Choose an avatar

Open **Appearance** and choose **Sigil only**, **Character with display**, or an earlier preset. The character option includes seven original shells. Preview the same avatar at card size and in the compact rendition. Enable animation to see the character’s idle poses and sigil motion. Open **Artwork** to upload PNG or JPEG artwork or generate a candidate using your configured image server.

Choose the border, layer geometry, work role, and responsibility. The top circles indicate responsibility. Configure kernel, architecture, weights terms, tier, and physical layers under **Model display details**. Model overrides are saved per model. Automatic tiers use Ollama’s reported parameter count where available. **Reading the sigil** identifies the source of the tier and shows recorded throughput for the selected run. The preview speed slider is a demonstration.

**Duplicate** offers the exact appearance, a lineage variant, or a new sigil. A lineage variant keeps the visual identity and changes the motion recipe. Copies have fresh run history, access, and schedules.

Bobbins run on the current installation. Android supports manual runs while Spindle is active. The design’s next milestones cover paired homes and remote capability routing.
