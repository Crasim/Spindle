//! Memory, the living plan and git in the desktop: the notes shown under replies, the Plan
//! side panel, the Memory settings tab and the project page's Git section.

use super::*;
use crate::plan::{Step, Update};

#[derive(Default)]
pub(super) struct PlanPane {
    /// Steps to highlight, from the plan note the user clicked.
    pub(super) highlight: Vec<usize>,
    revision: String,
    replace_goal: bool,
    revising: bool,
    pub(super) submitted: bool,
    editing: Option<(usize, String)>,
    adding: String,
    clearing: bool,
}

impl PlanPane {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, thread: &crate::state::ThreadInfo, goal: &str, ime: &mut android_ime::ComposerIme) -> Option<side_tabs::Pick> {
        let steps = &thread.steps;
        if !goal.is_empty() {
            ui.add(egui::Label::new(Text::new(goal).strong()).truncate()).on_hover_text(goal);
        }
        if let Some(change) = thread.plan_changes.last() {
            ui.add(egui::Label::new(Text::new(change).color(muted())).truncate()).on_hover_text(change);
        }
        if self.revising {
            ui.label(Text::new("Revise plan").strong());
            ime.multiline(ui, "plan-revision", &mut self.revision, |edit| {
                edit.desired_rows(3).desired_width(f32::INFINITY).hint_text("What should change?")
            });
            ui.checkbox(&mut self.replace_goal, "Replace the current objective");
            let mut apply = false;
            ui.horizontal(|ui| {
                apply = ui.add_enabled(!self.revision.trim().is_empty() && self.revision.len() <= 1500, egui::Button::new("Apply change")).clicked();
                if ui.button("Back").clicked() { self.revising = false; }
            });
            if apply { self.submitted = true; }
            return apply.then(|| side_tabs::Pick::RevisePlan { change: self.revision.trim().into(), replace_goal: self.replace_goal });
        }
        if ui.small_button("Revise plan…").on_hover_text("Change the plan while work is running").clicked() {
            self.revising = true;
        }
        let mut next = None;
        let done = steps.iter().filter(|s| s.done).count();
        if steps.is_empty() {
            ui.label(
                Text::new(
                    "No plan yet. Ask for one with /plan, let the model start one for \
                     multi-step work, or add steps here. The plan stays with this \
                     conversation when you change models.",
                )
                .color(muted())
                .size(13.),
            );
        } else {
            ui.add(
                egui::ProgressBar::new(done as f32 / steps.len() as f32)
                    .desired_height(6.)
                    .fill(accent()),
            );
            ui.weak(format!("{done} of {} done", steps.len()));
        }
        ui.add_space(6.);
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height(ui.available_height() - 90.)
            .show(ui, |ui| {
                for (i, step) in steps.iter().enumerate() {
                    let lit = self.highlight.contains(&i);
                    egui::Frame::new()
                        .fill(if lit {
                            highlight()
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(6, 3))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let mut checked = step.done;
                                if ui.checkbox(&mut checked, "").changed() {
                                    let mut s = steps.to_vec();
                                    s[i].done = checked;
                                    next = Some(s);
                                }
                                if let Some((at, text)) = &mut self.editing
                                    && *at == i
                                {
                                    let width = (ui.available_width() - 30.).max(40.);
                                    let r = ime.field(ui, ("plan-step", i), text, Default::default(),
                                        |edit| edit.desired_width(width));
                                    r.request_focus();
                                    if ui.input(|inp| inp.key_pressed(egui::Key::Enter))
                                        || r.lost_focus()
                                    {
                                        let mut s = steps.to_vec();
                                        s[i].text = text.trim().to_owned();
                                        if s[i].text.is_empty() {
                                            s.remove(i);
                                        }
                                        next = Some(s);
                                        self.editing = None;
                                    } else if ui
                                        .input(|inp| inp.key_pressed(egui::Key::Escape))
                                    {
                                        self.editing = None;
                                    }
                                    return;
                                }
                                let text = if step.done {
                                    Text::new(&step.text).strikethrough().color(muted())
                                } else {
                                    Text::new(&step.text).color(ink())
                                };
                                let r = ui.add(
                                    egui::Label::new(text)
                                        .wrap()
                                        .sense(egui::Sense::click()),
                                );
                                if r.on_hover_text("Click to edit").clicked() {
                                    self.editing = Some((i, step.text.clone()));
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if cross(ui, "Remove step") {
                                            let mut s = steps.to_vec();
                                            s.remove(i);
                                            next = Some(s);
                                        }
                                    },
                                );
                            });
                        });
                }
            });
        ui.add_space(6.);
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 50.).max(40.);
            let r = ime.field(ui, "plan-add-step", &mut self.adding, Default::default(),
                |edit| edit.hint_text("Add a step").desired_width(width));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Add").clicked() || enter) && !self.adding.trim().is_empty()
            {
                let mut s = steps.to_vec();
                s.push(Step {
                    text: std::mem::take(&mut self.adding).trim().to_owned(),
                    done: false,
                });
                next = Some(s);
            }
        });
        if !steps.is_empty() {
            ui.horizontal(|ui| {
                if self.clearing {
                    ui.label(Text::new("Clear every step?").color(Color::LIGHT_RED));
                    if ui.small_button("Clear").clicked() {
                        next = Some(vec![]);
                        self.clearing = false;
                    }
                    if ui.small_button("Keep").clicked() {
                        self.clearing = false;
                    }
                } else if ui.small_button("Clear plan").clicked() {
                    self.clearing = true;
                }
            });
        }
        next.map(side_tabs::Pick::PlanSteps)
    }

    pub(super) fn revision_accepted(&mut self) {
        self.revising = false;
        self.revision.clear();
        self.replace_goal = false;
        self.submitted = false;
    }
}

#[derive(Default)]
pub(super) struct MemoryPage {
    /// Memories to highlight, from the memory note the user clicked.
    pub(super) highlight: Vec<u64>,
    editing: Option<(u64, String)>,
    adding: String,
    personal: bool,
    /// Show only this project's memories (from the project menu or page).
    pub(super) filter: Option<PathBuf>,
}

fn project_name(path: &std::path::Path) -> String {
    crate::projects::existing(path)
        .ok()
        .flatten()
        .map(|p| p.name)
        .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| crate::projects::display_path(path))
}

/// Settings › Sandbox › Components: what the user is typing and confirming.
#[derive(Default)]
pub(super) struct ComponentsPane {
    adding: String,
    removing: bool,
}

#[derive(Default)]
pub(super) struct GitPane {
    folder: PathBuf,
    status: Option<Result<crate::git::Status, String>>,
    commit: Option<String>,
    confirm: bool,
    initializing: bool,
    feedback: String,
}

/// What the user did with a note under a reply.
pub(super) enum NoteAction {
    ShowMemory(Vec<u64>),
    UndoMemory(Vec<u64>),
    ShowPlan(Vec<usize>),
    ShowSandbox,
}

/// A small rounded note, like "Memory updated", that can be clicked.
fn pill(ui: &mut egui::Ui, icon: Icon, text: &str, hover: &str) -> egui::Response {
    let inner = egui::Frame::new()
        .fill(raised())
        .stroke(egui::Stroke::new(1_f32, line()))
        .corner_radius(14)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                symbol(ui, icon, accent());
                ui.label(Text::new(text).size(13.).color(ink()));
            });
        });
    let r = ui.interact(
        inner.response.rect,
        ui.id().with(("pill", text)),
        egui::Sense::click(),
    );
    if r.hovered() {
        ui.painter().rect_stroke(
            inner.response.rect,
            14.,
            egui::Stroke::new(1_f32, accent()),
            egui::StrokeKind::Inside,
        );
    }
    r.on_hover_text(hover)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn cross(ui: &mut egui::Ui, hover: &str) -> bool {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(20., 20.), egui::Sense::click());
    let color = if r.hovered() { ink() } else { muted() };
    let (c, d) = (rect.center(), 4.);
    let stroke = egui::Stroke::new(1.5_f32, color);
    ui.painter()
        .line_segment([c + egui::vec2(-d, -d), c + egui::vec2(d, d)], stroke);
    ui.painter()
        .line_segment([c + egui::vec2(-d, d), c + egui::vec2(d, -d)], stroke);
    r.on_hover_text(hover).clicked()
}

fn shorten(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(n).collect::<String>())
    }
}

/// Notes under a reply for the memories it saved and the plan changes it made.
pub(super) fn reply_notes(
    ui: &mut egui::Ui,
    turn: &crate::state::Turn,
    memories: &[crate::memory::Memory],
) -> Option<NoteAction> {
    let mut action = None;
    let kept: Vec<&crate::memory::Memory> = turn
        .remembered
        .iter()
        .filter_map(|id| memories.iter().find(|m| m.id == *id))
        .collect();
    if turn.remembered.is_empty() && turn.plan.is_none() && turn.tool.is_none() {
        return None;
    }
    ui.horizontal_wrapped(|ui| {
        if let Some(tool) = &turn.tool {
            let text = format!("{} · {}", tool.summary, tool.status);
            let sandbox = tool.summary.starts_with("Set up the sandbox")
                || tool.summary.starts_with("Install Python packages");
            let r = pill(
                ui,
                Icon::Tool,
                &text,
                if sandbox {
                    "Open Settings › Sandbox"
                } else {
                    "Click to copy"
                },
            );
            if r.clicked() {
                if sandbox {
                    action = Some(NoteAction::ShowSandbox);
                } else {
                    ui.ctx().copy_text(text.clone());
                }
            }
        }
        if !turn.remembered.is_empty() {
            if kept.is_empty() {
                ui.weak("Memory removed");
            } else {
                let more = if kept.len() > 1 {
                    format!(" (+{} more)", kept.len() - 1)
                } else {
                    String::new()
                };
                let text = format!("Memory updated · {}{more}", shorten(&kept[0].text, 48));
                if pill(ui, Icon::Memory, &text, "See what was saved").clicked() {
                    action = Some(NoteAction::ShowMemory(turn.remembered.clone()));
                }
                if ui
                    .small_button("Undo")
                    .on_hover_text("Forget what this reply saved")
                    .clicked()
                {
                    action = Some(NoteAction::UndoMemory(turn.remembered.clone()));
                }
            }
        }
        if let Some(update) = &turn.plan {
            let text = format!("Plan updated · {}", update.summary());
            if pill(ui, Icon::Plan, &text, "Open the plan and see the changes").clicked() {
                action = Some(NoteAction::ShowPlan(update.changed.clone()));
            }
        }
    });
    action
}

/// A tool result sent back to the model, shown collapsed instead of as a user message.
pub(super) fn tool_result(ui: &mut egui::Ui, text: &str) {
    let mut lines = text.lines();
    let head = lines.next().unwrap_or("Tool result");
    let head = head.split_once(" · ").map(|(_, rest)| rest).unwrap_or(head);
    let first = lines.clone().next().unwrap_or_default();
    let failed = first.starts_with("Error:")
        || first.starts_with("Denied")
        || first.starts_with("The user denied");
    egui::CollapsingHeader::new(
        Text::new(format!(
            "{} · {}{}",
            if text.starts_with(crate::engine::NUDGE) {
                "Spindle"
            } else {
                "Result"
            },
            shorten(head, 90),
            if failed { " · not done" } else { "" }
        ))
        .size(13.)
        .color(if failed {
            Color::from_rgb(230, 160, 90)
        } else {
            muted()
        }),
    )
    .id_salt(ui.id().with("tool-result"))
    .show(ui, |ui| {
        if ui
            .small_button("Copy")
            .on_hover_text("Copy this result")
            .clicked()
        {
            ui.ctx().copy_text(text.to_owned());
        }
        // The whole result, selectable, including a first line too long for the header.
        let rest: String = lines.collect::<Vec<_>>().join("\n");
        let body = if head.chars().count() > 90 {
            format!("{head}\n{rest}")
        } else {
            rest
        };
        egui::ScrollArea::vertical()
            .max_height(260.)
            .show(ui, |ui| {
                ui.add(egui::Label::new(Text::new(body).monospace().size(12.).color(ink())).wrap());
            });
    });
}

/// A diff coloured for reading: added lines green, removed red, hunk headers in the accent.
pub(super) fn diff_text(text: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::monospace(12.);
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            job.append("\n", 0., egui::TextFormat::simple(font.clone(), ink()));
        }
        let color = if line.starts_with("+ ") {
            Color::from_rgb(120, 200, 130)
        } else if line.starts_with("- ") {
            Color::from_rgb(230, 120, 120)
        } else if line.starts_with("@@") || line.starts_with("── ") {
            accent()
        } else {
            ink()
        };
        job.append(line, 0., egui::TextFormat::simple(font.clone(), color));
    }
    job
}

/// Text that shows what a tool call would change.
fn preview(call: &crate::tools::Call) -> Option<String> {
    let clip = |s: &str, sign: char| -> String {
        let mut out: Vec<String> = s.lines().take(30).map(|l| format!("{sign} {l}")).collect();
        if s.lines().count() > 30 {
            out.push(format!("{sign} …"));
        }
        out.join("\n")
    };
    match call {
        crate::tools::Call::Patch { find, replace, .. } => {
            Some(format!("{}\n{}", clip(find, '-'), clip(replace, '+')))
        }
        crate::tools::Call::Create { content, .. } => Some(clip(content, '+')),
        _ => None,
    }
}

/// The approval card for a tool call waiting on the user. Returns the command for the choice.
pub(super) fn approval_card(ui: &mut egui::Ui, req: &crate::state::Pending) -> Option<String> {
    use crate::policy::{Level, levels, warning};
    let mut choice = None;
    ui.add_space(10.);
    egui::Frame::new()
        .fill(raised())
        .stroke(egui::Stroke::new(1.5_f32, accent()))
        .corner_radius(12)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(720.));
            ui.horizontal(|ui| {
                symbol(ui, Icon::Shield, accent());
                ui.label(Text::new(req.call.call.summary()).strong().size(15.).color(ink()));
            });
            let scope = if req.risk == crate::policy::Risk::Browser {
                format!("Paired tab for chat #{}", req.chat)
            } else if matches!(req.call.call, crate::tools::Call::DesktopControl(_)) {
                "Local Windows desktop".into()
            } else {
                crate::projects::display_path(&req.target)
            };
            let permission = match &req.call.call {
                crate::tools::Call::HostShell { .. } => "Run a local shell command",
                crate::tools::Call::DesktopControl(_) => "Windows app access",
                _ => req.risk.label(),
            };
            ui.label(
                Text::new(format!(
                    "{} · {} · {} model ({})",
                    permission, scope, req.trust.label(), route(&req.model).1
                ))
                .size(12.)
                .color(muted()),
            );
            if let Some(text) = (!req.preview.is_empty()).then(|| req.preview.clone()).or_else(|| preview(&req.call.call)) {
                egui::Frame::new()
                    .fill(surface())
                    .corner_radius(6)
                    .inner_margin(8)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(220.).show(ui, |ui| {
                            ui.add(egui::Label::new(diff_text(&text)).wrap());
                        });
                    });
            }
            let offered = levels(req.risk, req.trust);
            ui.horizontal_wrapped(|ui| {
                for level in offered {
                    if req.risk == crate::policy::Risk::Browser && req.project.is_none() && *level == Level::Project {
                        continue;
                    }
                    let installing = req.risk == crate::policy::Risk::Install;
                    let text = match (&req.call.call, level, installing) {
                        (crate::tools::Call::LoadTool{..}, Level::Once, _) => "Load this tool".to_owned(),
                        (crate::tools::Call::Setup {}, Level::Once, _) => "Set up the sandbox".to_owned(),
                        (crate::tools::Call::BuildApk { .. }, Level::Once, _) => "Build APK".to_owned(),
                        (_, Level::Once, true) => "Install".to_owned(),
                        (_, Level::Conversation, true) => "Allow installs in this conversation".to_owned(),
                        _ => level.label().to_owned(),
                    };
                    let mut button = if *level == Level::Once && installing {
                        ui.add(egui::Button::new(Text::new(text).strong().color(Color::WHITE)).fill(accent()))
                    } else {
                        ui.button(text)
                    };
                    if let Some(w) = warning(req.risk, *level) {
                        button = button.on_hover_text(w);
                    }
                    if button.clicked() {
                        let word = match level {
                            Level::Once => "once",
                            Level::Conversation => "conversation",
                            Level::Project => "project",
                            Level::Always => "always",
                        };
                        choice = Some(format!("/approve {} {word}", req.id));
                    }
                }
                let deny = if req.risk == crate::policy::Risk::Install { "Not now" } else { "Deny" };
                if ui.button(deny).clicked() {
                    choice = Some(format!("/deny {}", req.id));
                }
            });
            if matches!(req.call.call, crate::tools::Call::HostShell { .. }) {
                ui.weak("Approval applies only to the command shown above.");
            } else if matches!(req.call.call, crate::tools::Call::DesktopControl(_)) {
                ui.weak("Approve only this action. Press Esc to revoke desktop access and invalidate pending approvals.");
            } else if req.risk == crate::policy::Risk::Native {
                ui.weak("Each build or test needs its own approval.");
            } else if req.risk == crate::policy::Risk::Browser {
                ui.weak("Standing approvals apply to this URL, model, and action type. Manage them in Computer use.");
            } else if req.risk == crate::policy::Risk::Connector {
                ui.weak("The selected model receives the results of this read.");
            } else if !offered.contains(&Level::Project) && req.risk != crate::policy::Risk::Install {
                ui.label(
                    Text::new(format!(
                        "Standing permission for this isn't offered to {} models. You can change a model's trust in Settings › Models › Advanced.",
                        req.trust.label()
                    ))
                    .size(12.)
                    .color(muted()),
                );
            }
        });
    choice
}

/// The bar under a conversation whose sandbox copy has changes not yet in the project, and the
/// indicator for code running in the sandbox. Returns a command.
/// The note under a reply whose edits Spindle applied to the project. Returns true when the
/// user clicks Undo.
pub(super) fn applied_note(ui: &mut egui::Ui, applied: &crate::state::Applied) -> bool {
    let mut undo = false;
    ui.add_space(4.);
    let failed = applied.keep.as_os_str().is_empty();
    let head = match &applied.undone {
        Some(text) => format!("Undone · {}", shorten(text, 110)),
        None => shorten(applied.summary.lines().next().unwrap_or_default(), 120),
    };
    ui.horizontal_wrapped(|ui| {
        symbol(
            ui,
            Icon::Shield,
            if failed {
                Color::from_rgb(230, 160, 90)
            } else {
                accent()
            },
        );
        egui::CollapsingHeader::new(Text::new(head).size(13.).color(if failed {
            Color::from_rgb(230, 160, 90)
        } else {
            muted()
        }))
        .id_salt(ui.id().with("applied-diff"))
        .show(ui, |ui| {
            for line in applied.summary.lines().skip(1) {
                ui.label(
                    Text::new(line)
                        .size(12.)
                        .color(Color::from_rgb(230, 160, 90)),
                );
            }
            egui::ScrollArea::vertical()
                .max_height(320.)
                .show(ui, |ui| {
                    let mut job = egui::text::LayoutJob::default();
                    let font = egui::FontId::monospace(12.);
                    for line in applied.diff.lines() {
                        let color = if line.starts_with("+ ") {
                            Color::from_rgb(120, 200, 130)
                        } else if line.starts_with("- ") {
                            Color::from_rgb(230, 120, 120)
                        } else if line.starts_with("@@") || line.starts_with("── ") {
                            accent()
                        } else {
                            muted()
                        };
                        job.append(
                            &format!("{line}\n"),
                            0.,
                            egui::TextFormat::simple(font.clone(), color),
                        );
                    }
                    ui.add(egui::Label::new(job).selectable(true));
                });
        });
        if !failed
            && applied.undone.is_none()
            && ui
                .small_button("Undo")
                .on_hover_text("Put the project files back as they were before this reply")
                .clicked()
        {
            undo = true;
        }
    });
    undo
}

pub(super) fn sandbox_bar(
    ui: &mut egui::Ui,
    changes: &[String],
    job: Option<&str>,
    applying: bool,
    discarding: &mut bool,
) -> Option<String> {
    let mut command = None;
    if let Some(job) = job {
        ui.add_space(8.);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(Text::new(format!("{job}…")).color(muted()).size(13.));
            if ui.small_button("Stop").clicked() {
                command = Some("/stop-run".to_owned());
            }
        });
    }
    if changes.is_empty() || applying {
        return command;
    }
    ui.add_space(8.);
    egui::Frame::new()
        .fill(raised())
        .stroke(egui::Stroke::new(1_f32, line()))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(720.));
            ui.horizontal(|ui| {
                symbol(ui, Icon::Shield, accent());
                let n = changes.len();
                ui.label(
                    Text::new(format!(
                        "{n} change{} in the sandbox copy, not yet in the project",
                        if n == 1 { "" } else { "s" }
                    ))
                    .color(ink())
                    .size(13.),
                )
                .on_hover_text(changes.join("\n"));
            });
            ui.horizontal(|ui| {
                if ui.button("Review and apply…").clicked() {
                    command = Some("/apply".to_owned());
                }
                if *discarding {
                    ui.label(
                        Text::new("Throw away the copy's changes?")
                            .color(Color::LIGHT_RED)
                            .size(12.),
                    );
                    if ui.small_button("Discard").clicked() {
                        command = Some("/discard".to_owned());
                        *discarding = false;
                    }
                    if ui.small_button("Keep").clicked() {
                        *discarding = false;
                    }
                } else if ui.small_button("Discard copy…").clicked() {
                    *discarding = true;
                }
            });
        });
    command
}

/// The project page's Permissions section: standing grants with Revoke, and recent actions.
pub(super) fn permissions_section(
    ui: &mut egui::Ui,
    state: &crate::state::State,
    project: &std::path::Path,
) -> Option<String> {
    use crate::policy::Holder;
    let chats: std::collections::BTreeSet<u64> = std::iter::once((state.chat_id, &state.thread))
        .chain(state.chats.iter().map(|(id, c)| (*id, &c.thread)))
        .filter(|(_, t)| t.project.as_deref() == Some(project))
        .map(|(id, _)| id)
        .collect();
    let grants: Vec<&crate::policy::Grant> = state
        .grants
        .iter()
        .filter(|g| match &g.holder {
            Holder::Project(p) => p == project,
            Holder::Conversation(id) => chats.contains(id),
            Holder::Always => false,
        })
        .collect();
    let mut command = None;
    ui.horizontal(|ui| {
        symbol(ui, Icon::Shield, muted());
        ui.label("Permissions");
    });
    if grants.is_empty() {
        ui.label(
            Text::new("Nothing allowed beyond reading project files. Spindle asks first.")
                .size(12.)
                .color(muted()),
        );
    }
    for g in grants {
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(Text::new(g.describe()).size(12.).color(ink())).wrap());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if cross(ui, "Revoke") {
                    command = Some(format!("/revoke {}", g.id));
                }
            });
        });
    }
    let actions: Vec<&crate::state::ActionRecord> = state
        .actions
        .iter()
        .rev()
        .filter(|a| chats.contains(&a.chat))
        .take(30)
        .collect();
    if !actions.is_empty() {
        egui::CollapsingHeader::new(Text::new("Action log").size(12.))
            .id_salt("action-log")
            .show(ui, |ui| {
                for a in actions {
                    let when = chrono::DateTime::from_timestamp(a.time, 0)
                        .map(|d| {
                            d.with_timezone(&chrono::Local)
                                .format("%b %-d %H:%M")
                                .to_string()
                        })
                        .unwrap_or_default();
                    ui.label(
                        Text::new(format!("{when} · {}", a.summary))
                            .size(12.)
                            .color(ink()),
                    );
                    ui.label(
                        Text::new(format!(
                            "{} · {}{}",
                            route(&a.model).1,
                            a.decision,
                            if a.outcome.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", a.outcome)
                            }
                        ))
                        .size(11.)
                        .color(if a.ok {
                            muted()
                        } else {
                            Color::from_rgb(230, 160, 90)
                        }),
                    );
                }
            });
    }
    command
}

/// A note in the conversation for plan edits the user made between messages.
pub(super) fn user_plan_note(ui: &mut egui::Ui, update: &Update) -> Option<NoteAction> {
    let mut action = None;
    ui.add_space(8.);
    ui.vertical_centered(|ui| {
        let text = format!("You updated the plan · {}", update.summary());
        if pill(ui, Icon::Plan, &text, "Open the plan and see the changes").clicked() {
            action = Some(NoteAction::ShowPlan(update.changed.clone()));
        }
    });
    action
}

impl Desktop {
    pub(super) fn note_action(&mut self, action: NoteAction) {
        match action {
            NoteAction::ShowMemory(ids) => {
                self.show_memories(None);
                self.memory.highlight = ids;
            }
            NoteAction::UndoMemory(ids) => {
                for id in ids {
                    self.send(format!("/forget {id}"));
                }
            }
            NoteAction::ShowSandbox => {
                self.preferences_tab = false;
                self.models_tab = false;
                self.memory_tab = false;
                self.sandbox_tab = true;
                self.integration_tab = None;
                self.sync_tab = false;
                self.open_settings();
            }
            NoteAction::ShowPlan(changed) => {
                self.plan.highlight = changed;
                self.reader.open = true;
                self.reader.tab = side_tabs::Tab::Plan;
            }
        }
    }

    pub(super) fn set_steps(&mut self, steps: Vec<Step>) {
        self.plan.highlight.clear();
        self.send(format!(
            "/plan-steps {}",
            serde_json::to_string(&steps).expect("Serializable plan")
        ));
    }

    /// Settings › Sandbox: Spindle's Python, the sandbox check and whether running code is on.
    pub(super) fn sandbox_settings(&mut self, ui: &mut egui::Ui) {
        if cfg!(target_os = "android") {
            ui.heading("Android builds");
            ui.label("Spindle can compile and sign small Java Android apps in an isolated service on this device.");
            ui.label("Ask for a Java APK in chat, then approve Build APK. Use Download on the resulting file.");
            ui.weak("The built-in profile uses Android framework APIs and a programmatic UI.");
            return;
        }
        let runtime = crate::runtime::installed();
        let report = self.view.state.sandbox.clone();
        let job = self.view.job.clone();
        let mut command: Option<String> = None;
        let card = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            egui::Frame::new()
                .fill(panel())
                .inner_margin(16)
                .corner_radius(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui);
                });
            ui.add_space(12.);
        };
        card(ui, &mut |ui| {
            ui.horizontal(|ui| {
                symbol(ui, Icon::Shield, accent());
                ui.label(Text::new("Sandbox").size(19.).strong());
            });
            let on = crate::execution::ready(&self.view.state);
            ui.label(
                Text::new(if on {
                    "Running code is on."
                } else {
                    "Running code is off."
                })
                .strong()
                .color(if on { accent() } else { ink() }),
            );
            ui.label(
                Text::new(
                    "Models run tests and scripts in a sandbox: a copy of the project, Spindle's own Python, no network and no access to your other files. Edits reach the project when a reply finishes (with Undo), or when you approve, as set below. Anything outside the sandbox asks first.",
                )
                .size(13.)
                .color(muted()),
            );
            let mut prefs = self.view.state.preferences.clone();
            if ui
                .checkbox(
                    &mut prefs.auto_apply,
                    "Apply the model's edits to the project when it finishes a reply",
                )
                .on_hover_text(
                    "Each reply shows what changed, with Undo. Off: you approve each apply.",
                )
                .changed()
            {
                command = Some(format!(
                    "/preferences {}",
                    serde_json::to_string(&prefs).unwrap()
                ));
            }
            if ui
                .checkbox(
                    &mut prefs.block_web,
                    "Block web access for the model browser",
                )
                .on_hover_text("Blocks public web search, public pages, connected tabs and internet requests from local tests. Local page tests can still run.")
                .changed()
            {
                command = Some(format!("/preferences {}", serde_json::to_string(&prefs).unwrap()));
            }
            if !on {
                ui.label(
                    Text::new(crate::execution::unavailable(&self.view.state))
                        .size(13.)
                        .color(ink()),
                );
            }
            if let Some(job) = &job {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("{job}…"));
                });
            }
        });
        card(ui, &mut |ui| {
            ui.horizontal(|ui| {
                ui.label(Text::new("Sandbox check").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            runtime.is_some() && job.is_none(),
                            egui::Button::new("Run the check"),
                        )
                        .on_hover_text("Tries to break each promise the sandbox makes")
                        .clicked()
                    {
                        command = Some("/sandbox-check".into());
                    }
                });
            });
            if let Some(r) = &report
                && ui
                    .small_button("Copy report")
                    .on_hover_text("Copy the check results as text")
                    .clicked()
            {
                ui.ctx().copy_text(r.text());
            }
            match &report {
                None => {
                    ui.weak("Not run yet.");
                }
                Some(r) => {
                    let when = chrono::DateTime::from_timestamp(r.time, 0)
                        .map(|d| {
                            d.with_timezone(&chrono::Local)
                                .format("%b %-d %H:%M")
                                .to_string()
                        })
                        .unwrap_or_default();
                    ui.label(
                        Text::new(format!("{} · {when}", r.backend))
                            .size(12.)
                            .color(muted()),
                    );
                    for c in &r.checks {
                        ui.horizontal(|ui| {
                            let (mark, color) = if c.passed {
                                ("✓", accent())
                            } else {
                                ("✗", Color::LIGHT_RED)
                            };
                            ui.label(Text::new(mark).color(color).strong());
                            ui.label(Text::new(&c.name).size(13.).color(ink()));
                            ui.label(Text::new(&c.detail).size(12.).color(muted()));
                        });
                    }
                    if !r.problem.is_empty() {
                        ui.label(Text::new(&r.problem).size(13.).color(ink()));
                    }
                }
            }
        });
        if crate::browser::SUPPORTED {
            card(ui, &mut |ui| self.model_browser_settings(ui));
        }
        let data = crate::runtime::data_dir();
        if crate::native::supported() {
            card(ui, &mut |ui| {
                ui.label(Text::new("Native builds").size(16.).strong());
                ui.label("Compile and test Rust, C and C++. Each build or retest asks for approval. Tested binaries appear in the chat's Build outputs for export.");
                ui.weak(if crate::rust_build::available() {"Rust stable detected for sandboxed .rs builds"} else {"For Rust builds, install the stable Rust host toolchain"});
                if crate::native::compiler(&data).is_some() {
                    ui.weak(format!("{} installed", crate::native::compiler_label()));
                } else if ui.add_enabled(job.is_none(), egui::Button::new(format!("Install {}", crate::native::compiler_label()))).clicked() {
                    command = Some("/compiler-install".into());
                }
            });
        }
        let packages = crate::packages::list(&data);
        let mut install: Option<String> = None;
        card(ui, &mut |ui| {
            ui.label(Text::new("Components").size(16.).strong());
            ui.label(
                Text::new("What Spindle has installed for running code. All of it lives in Spindle's own folder and is used only inside the sandbox.")
                    .size(12.)
                    .color(muted()),
            );
            ui.add_space(6.);
            // Spindle's Python
            ui.horizontal(|ui| {
                ui.label(Text::new("Python").strong().color(ink()));
                match &runtime {
                    Some(rt) => {
                        ui.label(
                            Text::new(format!("{} · installed", rt.version))
                                .size(12.)
                                .color(accent()),
                        );
                    }
                    None => {
                        ui.label(Text::new("not installed").size(12.).color(muted()));
                    }
                }
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| match &runtime {
                        Some(_) if self.components.removing => {
                            if ui.small_button("Keep").clicked() {
                                self.components.removing = false;
                            }
                            if ui.small_button("Remove").clicked() {
                                command = Some("/remove-runtime".into());
                                self.components.removing = false;
                            }
                            ui.label(
                                Text::new("Remove Python? Running code turns off.")
                                    .size(12.)
                                    .color(Color::LIGHT_RED),
                            );
                        }
                        Some(_) => {
                            if ui
                                .add_enabled(job.is_none(), egui::Button::new("Remove…").small())
                                .clicked()
                            {
                                self.components.removing = true;
                            }
                        }
                        None => {
                            if ui
                                .add_enabled(
                                    job.is_none(),
                                    egui::Button::new(format!(
                                        "Set up Python {}",
                                        crate::runtime::PIN.version
                                    )),
                                )
                                .clicked()
                            {
                                command = Some("/runtime-install".into());
                            }
                        }
                    },
                );
            });
            ui.label(
                Text::new(match &runtime {
                    Some(rt) => crate::projects::display_path(&rt.home),
                    None => format!(
                        "Downloads python-build-standalone {} (about 37 MB) from github.com, checked against a pinned SHA-256.",
                        crate::runtime::PIN.release
                    ),
                })
                .size(12.)
                .color(muted()),
            );
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(Text::new("spindle_docs").strong().color(ink()));
                ui.label(Text::new("built in").size(12.).color(accent()));
            });
            ui.label(
                Text::new(
                    "Writes .docx and .xlsx files from scripts with only the standard library.",
                )
                .size(12.)
                .color(muted()),
            );
            ui.separator();
            ui.label(Text::new("Python packages").strong().color(ink()));
            if packages.is_empty() {
                ui.label(
                    Text::new("None installed. Models ask before installing one, or add one here.")
                        .size(12.)
                        .color(muted()),
                );
            }
            for p in &packages {
                ui.horizontal(|ui| {
                    ui.label(Text::new(&p.name).color(ink()).size(13.))
                        .on_hover_text(&p.summary);
                    ui.label(
                        Text::new(format!(
                            "{} · {}",
                            p.version,
                            crate::packages::human(p.bytes)
                        ))
                        .size(12.)
                        .color(muted()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if cross(ui, "Remove this package") && job.is_none() {
                            command = Some(format!("/remove-package {}", p.name));
                        }
                    });
                });
            }
            ui.add_enabled_ui(runtime.is_some() && job.is_none(), |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.components.adding)
                            .hint_text("Package name, e.g. python-docx")
                            .desired_width(220.),
                    );
                    if ui
                        .button("Install")
                        .on_hover_text("From pypi.org, prebuilt packages only")
                        .clicked()
                    {
                        let specs: Vec<String> = self
                            .components
                            .adding
                            .split_whitespace()
                            .map(str::to_owned)
                            .collect();
                        if !specs.is_empty() && specs.iter().all(|s| crate::packages::valid(s)) {
                            install = Some(specs.join(" "));
                            self.components.adding.clear();
                        }
                    }
                });
                let missing: Vec<&(&str, &str)> = crate::packages::SUGGESTED
                    .iter()
                    .filter(|(name, _)| !packages.iter().any(|p| p.name.eq_ignore_ascii_case(name)))
                    .collect();
                if !missing.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(Text::new("Suggested:").size(12.).color(muted()));
                        for (name, why) in missing {
                            if ui.small_button(*name).on_hover_text(*why).clicked() {
                                install = Some((*name).to_owned());
                            }
                        }
                    });
                }
            });
        });
        if let Some(specs) = install {
            command = Some(format!("/install-packages {specs}"));
        }
        if let Some(cmd) = command {
            self.send(cmd);
        }
    }

    fn model_browser_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(Text::new("Model browser").size(16.).strong());
        ui.label("Spindle starts Edge, Chrome or Chromium with a fresh profile for web browsing and HTML tests.");
        let saved = &self.view.state.preferences.browser_path;
        let mut path = self
            .browser_path_draft
            .clone()
            .unwrap_or_else(|| saved.clone());
        ui.label("Browser executable");
        ui.horizontal(|ui| {
            let browse_width = if cfg!(windows) { 90. } else { 0. };
            ui.add(
                egui::TextEdit::singleline(&mut path)
                    .desired_width((ui.available_width() - browse_width).max(50.))
                    .hint_text("Automatic detection"),
            );
            #[cfg(windows)]
            if ui.button("Browse…").clicked()
                && let Some(file) = rfd::FileDialog::new()
                    .set_title("Choose a Chromium browser executable")
                    .add_filter("Executables", &["exe"])
                    .pick_file()
            {
                path = file.to_string_lossy().into_owned();
            }
        });
        let configured = path.trim();
        let changed = configured != saved;
        let found = crate::browser::resolve(configured);
        match &found {
            Ok(exe) => {
                let label = if configured.is_empty() { "Detected" } else { "Selected" };
                ui.weak(format!("{label} · {}", crate::projects::display_path(exe)));
            }
            Err(error) => {
                if configured.is_empty() {
                    ui.label(error.to_string());
                } else {
                    ui.label(format!("Browser path unavailable. {:#}", error.root_cause()));
                }
            }
        }
        let mut command = None;
        let mut reset = false;
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(
                changed && (configured.is_empty() || found.is_ok()),
                egui::Button::new("Save browser"),
            ).clicked() {
                command = Some(format!("/browser-path {}", serde_json::json!(configured)));
            }
            if ui.add_enabled(
                !configured.is_empty() || !saved.is_empty(),
                egui::Button::new("Use auto-detection"),
            ).clicked() {
                command = Some("/browser-path \"\"".into());
                reset = true;
            }
            if ui.add_enabled(self.view.job.is_none(), egui::Button::new("Check browser")).clicked() {
                command = Some(format!("/browser-check {}", serde_json::json!(configured)));
            }
        });
        let last_check = self.view.browser_check.as_ref()
            .filter(|(checked, _)| checked == configured);
        if let Some((_, result)) = last_check {
            ui.label(result);
        }
        if found.is_err() || last_check.is_some_and(|(_, result)| result.starts_with("Browser check failed")) {
            ui.label("Install a compatible browser, then check again.");
            ui.horizontal_wrapped(|ui| {
                ui.hyperlink_to("Download Chrome", "https://www.google.com/chrome/");
                ui.hyperlink_to("Download Edge", "https://www.microsoft.com/edge/download");
                ui.hyperlink_to("Get Chromium", "https://www.chromium.org/getting-involved/download-chromium/");
            });
        }
        if reset {
            path.clear();
        }
        self.browser_path_draft = (path != self.view.state.preferences.browser_path).then_some(path);
        if let Some(command) = command {
            self.send(command);
        }
    }

    /// Show the Memory settings, optionally only one project's memories.
    pub(super) fn show_memories(&mut self, project: Option<PathBuf>) {
        self.memory.filter = project;
        self.memory.highlight.clear();
        self.preferences_tab = false;
        self.models_tab = false;
        self.memory_tab = true;
        self.sandbox_tab = false;
        self.integration_tab = None;
        self.sync_tab = false;
            self.open_settings();
    }

    /// Settings › Memory: the switches, and everything Spindle remembers to read, edit or forget.
    pub(super) fn memory_settings(&mut self, ui: &mut egui::Ui) {
        let state = &self.view.state;
        let mut prefs = state.preferences.clone();
        let memories = state.memories.clone();
        let off = state.memory_off.clone();
        let filter = self.memory.filter.clone();
        let mut library: Vec<(PathBuf, String)> = self
            .projects
            .recent
            .iter()
            .map(|p| (p.path.clone(), p.name.clone()))
            .collect();
        for m in &memories {
            if let Some(p) = &m.project
                && !library.iter().any(|(path, _)| path == p)
            {
                library.push((p.clone(), project_name(p)));
            }
        }
        let mut commands: Vec<String> = vec![];
        let mut prefs_changed = false;
        if let Some(path) = &filter {
            ui.horizontal(|ui| {
                symbol(ui, Icon::Folder, accent());
                ui.label(
                    Text::new(format!("{} · memories", project_name(path)))
                        .size(19.)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("All memories").clicked() {
                        self.memory.filter = None;
                    }
                });
            });
            let mut on = !off.contains(path);
            let enabled = prefs.memory && prefs.project_memory;
            if ui
                .add_enabled(
                    enabled,
                    egui::Checkbox::new(&mut on, "Use memory in this project's chats"),
                )
                .changed()
            {
                commands.push(format!(
                    "/memory-project {}",
                    serde_json::to_string(&(path, on)).expect("Serializable path")
                ));
            }
            if !enabled {
                ui.weak("Project memory is turned off for every project in All memories.");
            }
            ui.add_space(8.);
        } else {
            egui::Frame::new()
                .fill(panel())
                .inner_margin(16)
                .corner_radius(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        symbol(ui, Icon::Memory, accent());
                        ui.label(Text::new("Memory").size(19.).strong());
                    });
                    prefs_changed |= ui
                        .checkbox(&mut prefs.memory, "Remember things between conversations")
                        .changed();
                    ui.add_enabled_ui(prefs.memory, |ui| {
                        ui.indent("memory-scopes", |ui| {
                            prefs_changed |= ui
                                .checkbox(&mut prefs.personal_memory, "Personal memory")
                                .on_hover_text("Facts about you, used in every chat")
                                .changed();
                            prefs_changed |= ui
                                .checkbox(&mut prefs.project_memory, "Project memory")
                                .on_hover_text(
                                    "Facts about a project, used only in that project's chats",
                                )
                                .changed();
                            if !library.is_empty() {
                                ui.add_enabled_ui(prefs.project_memory, |ui| {
                                    ui.indent("memory-projects", |ui| {
                                        for (path, name) in &library {
                                            let mut on = !off.contains(path);
                                            ui.horizontal(|ui| {
                                                if ui.checkbox(&mut on, name).changed() {
                                                    commands.push(format!(
                                                        "/memory-project {}",
                                                        serde_json::to_string(&(path, on))
                                                            .expect("Serializable path")
                                                    ));
                                                }
                                                let count = memories
                                                    .iter()
                                                    .filter(|m| m.project.as_ref() == Some(path))
                                                    .count();
                                                if ui
                                                    .small_button(format!("{count} saved"))
                                                    .on_hover_text("See this project's memories")
                                                    .clicked()
                                                {
                                                    self.memory.filter = Some(path.clone());
                                                }
                                            });
                                        }
                                    });
                                });
                            }
                        });
                    });
                    ui.label(
                        Text::new(
                            "Models save short facts when you share something worth keeping, and \
                             each reply that saves one shows a note you can undo. Turning a memory \
                             off keeps what is saved but stops chats from using or adding to it. \
                             Memories are stored encrypted with your session and leave this \
                             computer only in requests to the model you chose.",
                        )
                        .size(13.)
                        .color(muted()),
                    );
                });
            ui.add_space(12.);
        }
        let mut groups: Vec<(Option<PathBuf>, Vec<&crate::memory::Memory>)> = vec![];
        for m in memories.iter().rev() {
            if filter.is_some() && m.project != filter {
                continue;
            }
            match groups.iter_mut().find(|(p, _)| *p == m.project) {
                Some((_, list)) => list.push(m),
                None => groups.push((m.project.clone(), vec![m])),
            }
        }
        groups.sort_by_key(|(p, _)| p.is_some());
        if groups.is_empty() {
            ui.weak("Nothing remembered yet.");
        }
        for (scope, list) in &groups {
            let paused = match scope {
                None => !(prefs.memory && prefs.personal_memory),
                Some(p) => !(prefs.memory && prefs.project_memory) || off.contains(p),
            };
            if filter.is_none() {
                let title = match scope {
                    None => "Personal".to_owned(),
                    Some(path) => format!("Project · {}", project_name(path)),
                };
                ui.horizontal(|ui| {
                    ui.label(Text::new(title).strong().color(muted()));
                    if paused {
                        ui.weak("· off");
                    }
                });
            } else if paused {
                ui.weak("Off: these are kept but not used.");
            }
            for m in list {
                let lit = self.memory.highlight.contains(&m.id);
                egui::Frame::new()
                    .fill(if lit { highlight() } else { raised() })
                    .stroke(egui::Stroke::new(
                        1_f32,
                        if lit { accent() } else { line() },
                    ))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if let Some((id, text)) = &mut self.memory.editing
                            && *id == m.id
                        {
                            ui.add(
                                egui::TextEdit::multiline(text)
                                    .desired_rows(2)
                                    .desired_width(f32::INFINITY),
                            );
                            let (save, cancel) = ui
                                .horizontal(|ui| {
                                    (ui.button("Save").clicked(), ui.button("Cancel").clicked())
                                })
                                .inner;
                            if save {
                                commands.push(format!(
                                    "/memory-edit {}",
                                    serde_json::to_string(&(m.id, text.trim()))
                                        .expect("Serializable memory")
                                ));
                            }
                            if save || cancel {
                                self.memory.editing = None;
                            }
                            return;
                        }
                        ui.horizontal(|ui| {
                            let color = if paused { muted() } else { ink() };
                            ui.add(egui::Label::new(Text::new(&m.text).color(color)).wrap());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if cross(ui, "Forget this") {
                                        commands.push(format!("/forget {}", m.id));
                                    }
                                    if ui.small_button("Edit").clicked() {
                                        self.memory.editing = Some((m.id, m.text.clone()));
                                    }
                                    let when = chrono::DateTime::from_timestamp(m.saved, 0)
                                        .map(|d| {
                                            d.with_timezone(&chrono::Local)
                                                .format("%b %-d")
                                                .to_string()
                                        })
                                        .unwrap_or_default();
                                    ui.weak(when);
                                },
                            );
                        });
                    });
            }
            ui.add_space(10.);
        }
        ui.add_space(6.);
        ui.label(Text::new("Add a memory").strong().color(muted()));
        let current = state.thread.project.clone();
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.memory.adding)
                    .hint_text("Something Spindle should remember")
                    .desired_width((ui.available_width() - 190.).max(160.)),
            );
            if filter.is_none() && current.is_some() {
                ui.checkbox(&mut self.memory.personal, "Personal");
            }
            if ui.button("Remember").clicked() && !self.memory.adding.trim().is_empty() {
                let text = std::mem::take(&mut self.memory.adding).trim().to_owned();
                commands.push(match (&filter, &current) {
                    (Some(path), _) => format!(
                        "/remember-in {}",
                        serde_json::to_string(&(path, &text)).expect("Serializable memory")
                    ),
                    (None, Some(_)) if !self.memory.personal => format!("/remember {text}"),
                    _ => format!("/remember-personal {text}"),
                });
            }
        });
        if filter.is_none() && current.is_some() {
            ui.weak("Without Personal, the memory belongs to the open conversation's project.");
        }
        if prefs_changed {
            commands.insert(
                0,
                format!(
                    "/preferences {}",
                    serde_json::to_string(&prefs).expect("Serializable preferences")
                ),
            );
        }
        for cmd in commands {
            self.send(cmd);
        }
    }
}

impl GitPane {
    /// The Git section of a project page: status, history and a confirmed commit.
    /// Returns true when the user asks for a review of the changes.
    pub(super) fn show(&mut self, ui: &mut egui::Ui, folder: &std::path::Path) -> bool {
        if self.folder != folder {
            *self = GitPane {
                folder: folder.to_owned(),
                ..Default::default()
            };
        }
        let mut review = false;
        if self.status.is_none() {
            self.status = Some(crate::git::status(folder).map_err(|e| e.to_string()));
        }
        ui.horizontal(|ui| {
            symbol(ui, Icon::Branch, muted());
            ui.label("Git");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Refresh").clicked() {
                    self.status = None;
                    self.feedback.clear();
                }
            });
        });
        let status = match self.status.clone() {
            Some(Ok(s)) => s,
            Some(Err(e)) => {
                ui.label(Text::new(e).size(12.).color(muted()));
                return false;
            }
            None => return false,
        };
        if !status.repository {
            ui.label(
                Text::new("This folder is not a git repository.")
                    .size(12.)
                    .color(muted()),
            );
            if self.initializing {
                ui.horizontal(|ui| {
                    if ui.small_button("Create repository").clicked() {
                        self.feedback = match crate::git::init(folder) {
                            Ok(()) => "Repository created".into(),
                            Err(e) => e.to_string(),
                        };
                        self.initializing = false;
                        self.status = None;
                    }
                    if ui.small_button("Cancel").clicked() {
                        self.initializing = false;
                    }
                });
            } else if ui.small_button("Initialize…").clicked() {
                self.initializing = true;
            }
            return false;
        }
        let mut head = format!("On {}", status.branch);
        if !status.ahead_behind.is_empty() {
            head.push_str(&format!(" · {}", status.ahead_behind));
        }
        ui.label(Text::new(head).size(12.).color(muted()));
        if status.changes.is_empty() {
            ui.label(Text::new("No uncommitted changes").size(12.).color(muted()));
        } else {
            ui.label(
                Text::new(format!(
                    "{} uncommitted change{}",
                    status.changes.len(),
                    if status.changes.len() == 1 { "" } else { "s" }
                ))
                .size(12.),
            );
            egui::ScrollArea::vertical()
                .id_salt("git-changes")
                .max_height(120.)
                .show(ui, |ui| {
                    for change in &status.changes {
                        ui.horizontal(|ui| {
                            ui.label(Text::new(change.describe()).size(11.).color(accent()));
                            ui.add(
                                egui::Label::new(Text::new(&change.path).size(12.).color(ink()))
                                    .truncate(),
                            )
                            .on_hover_text(&change.path);
                        });
                    }
                });
            ui.horizontal(|ui| {
                if ui
                    .small_button("Review")
                    .on_hover_text("Ask the model to review the uncommitted changes")
                    .clicked()
                {
                    review = true;
                }
                if self.commit.is_none() && ui.small_button("Commit…").clicked() {
                    self.commit = Some(String::new());
                    self.confirm = false;
                }
            });
        }
        if let Some(message) = &mut self.commit {
            ui.add(
                egui::TextEdit::multiline(message)
                    .hint_text("Commit message")
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            let ready = !message.trim().is_empty();
            if self.confirm {
                ui.label(
                    Text::new(format!(
                        "Commit all {} changes on {}?",
                        status.changes.len(),
                        status.branch
                    ))
                    .size(12.)
                    .color(ink()),
                );
            }
            let mut done = None;
            ui.horizontal(|ui| {
                let label = if self.confirm {
                    "Confirm commit"
                } else {
                    "Commit"
                };
                if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
                    if self.confirm {
                        done = Some(crate::git::commit(folder, message));
                    } else {
                        self.confirm = true;
                    }
                }
                if ui.small_button("Cancel").clicked() {
                    done = Some(Err(anyhow::anyhow!("")));
                }
            });
            match done {
                Some(Ok(summary)) => {
                    self.feedback = format!("Committed {summary}");
                    self.commit = None;
                    self.status = None;
                }
                Some(Err(e)) => {
                    self.feedback = e.to_string();
                    if self.feedback.is_empty() {
                        self.commit = None;
                    }
                }
                None => {}
            }
        }
        if !self.feedback.is_empty() {
            ui.label(Text::new(&self.feedback).size(12.).color(muted()));
        }
        if !status.log.is_empty() {
            egui::CollapsingHeader::new(Text::new("Recent commits").size(12.))
                .id_salt("git-log")
                .show(ui, |ui| {
                    for (hash, subject, when) in &status.log {
                        ui.horizontal(|ui| {
                            ui.label(Text::new(hash).monospace().size(11.).color(muted()));
                            ui.add(egui::Label::new(Text::new(subject).size(12.)).truncate())
                                .on_hover_text(format!("{subject} · {when}"));
                        });
                    }
                });
        }
        review
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Pos2};
    use std::collections::BTreeMap;

    fn run(app: &mut Desktop, ctx: &egui::Context, events: Vec<Event>) -> BTreeMap<String, Pos2> {
        run_at(app, ctx, events, egui::vec2(1280., 1200.))
    }

    fn run_at(
        app: &mut Desktop,
        ctx: &egui::Context,
        events: Vec<Event>,
        size: egui::Vec2,
    ) -> BTreeMap<String, Pos2> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let mut frame = eframe::Frame::_new_kittest();
        let output = ctx.run(input, |ctx| eframe::App::update(app, ctx, &mut frame));
        fn collect(shape: &egui::Shape, labels: &mut BTreeMap<String, Pos2>) {
            match shape {
                egui::Shape::Text(t) => {
                    labels.insert(t.galley.text().into(), t.pos + t.galley.size() * 0.5);
                }
                egui::Shape::Vec(children) => children.iter().for_each(|c| collect(c, labels)),
                _ => {}
            }
        }
        let mut labels = BTreeMap::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut labels);
        }
        labels
    }

    fn click(app: &mut Desktop, ctx: &egui::Context, pos: Pos2) -> BTreeMap<String, Pos2> {
        for pressed in [true, false] {
            run(
                app,
                ctx,
                vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        run(app, ctx, vec![])
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn computer_use_browser_card_manages_the_connected_browser() {
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.page = Page::Settings;
        app.integration_tab = Some(settings::IntegrationTab::ComputerUse);
        app.view.browser_clients.push(crate::browser_native::BrowserClient {
            id: "sample".into(),
            browser: crate::browser::LinkBrowser::Edge,
            version: Some("0.5.4".into()),
            ready: true,
            has_tab: true,
            guided: true,
            managed: false,
            located: None,
        });
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(labels.contains_key("Microsoft Edge") && labels.contains_key("Google Chrome"));
        let labels = click(&mut app, &ctx, labels["Manage"]);
        assert!(labels.contains_key("Update Browser Link"));
        assert!(labels.contains_key("Back"));
        for text in ["Allow all", "Allowlist", "Blocklist", "Block all", "Saved model permissions"] {
            assert!(labels.contains_key(text), "missing {text}");
        }
        let labels = click(&mut app, &ctx, labels["Blocklist"]);
        assert!(commands.try_recv().is_ok_and(|command| command == "/browser-mode \"blocklist\""));
        click(&mut app, &ctx, labels["On"]);
        assert!(std::iter::from_fn(|| commands.try_recv().ok()).any(|sent| sent == "/browser-control edge off"));
    }

    #[test]
    fn blank_browser_offers_the_requested_website_in_the_chat() {
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.page = Page::Chat;
        let chat = app.view.state.chat_id;
        app.view.state.browser_reads.insert(chat, crate::state::BrowserRead {
            id: 9, destination: Some(reqwest::Url::parse("https://example.com/cart").unwrap()),
            approved_origin: None, locating: None, model: "Example model".into(), steps: 0, read_only_chat: true,
            call: crate::tools::extract(crate::engine::BROWSER_TOOL_EXAMPLE).1.unwrap().unwrap(),
        });
        app.view.browser_clients.push(crate::browser_native::BrowserClient {
            id: "sample".into(), browser: crate::browser::LinkBrowser::Edge, version: None, ready: true, guided: true, has_tab: false, managed: false, located: None,
        });
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(labels.contains_key("Update Browser Link to continue"));
        assert!(labels.contains_key("Update Browser Link for Microsoft Edge"));
        assert!(!labels.contains_key("Allow this site"));
        app.view.browser_clients[0].version = Some(crate::browser_link::extension_version().into());
        let labels = run(&mut app, &ctx, vec![]);
        assert!(labels.contains_key("https://example.com"));
        assert!(!labels.contains_key("Set up Browser Link"));
        click(&mut app, &ctx, labels["Allow this site"]);
        let expected = format!("/browser-site read-site {chat} 9 https://example.com");
        assert!(std::iter::from_fn(|| commands.try_recv().ok()).any(|sent| sent == expected));
        // A connection left behind by an older Spindle browser does not
        // replace the regular-browser setup or trigger a site approval.
        app.view.browser_clients[0].managed = true;
        let labels = run(&mut app, &ctx, vec![]);
        assert!(labels.contains_key("Connect your regular browser"));
        assert!(!labels.contains_key("Allow all"));
        assert!(!labels.contains_key("Open Spindle browser"));
        click(&mut app, &ctx, labels["Cancel"]);
        let expected = format!("/browser-site read-deny {chat} 9");
        assert!(std::iter::from_fn(|| commands.try_recv().ok()).any(|sent| sent == expected));
    }

    #[test]
    fn browser_access_choices_stay_in_the_requesting_chat() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.page = Page::Chat;
        let chat = app.view.state.chat_id;
        let site = "https://example.com";
        app.view.browser_pairing = Some((chat, false));
        app.view.browser_tab_origin = Some(site.into());
        app.view.browser_site_request = Some(site.into());
        run(&mut app, &ctx, vec![]);
        for (label, choice) in [("Allow this site", "site"), ("Allow all", "all"), ("Deny", "deny")] {
            let labels = run(&mut app, &ctx, vec![]);
            assert!(labels.contains_key(site));
            click(&mut app, &ctx, labels[label]);
            let command = format!("/browser-site {choice} {chat} {site}");
            assert!(std::iter::from_fn(|| commands.try_recv().ok()).any(|sent| sent == command));
        }
        app.view.state.chat_id += 1;
        let labels = run(&mut app, &ctx, vec![]);
        assert!(!labels.contains_key("Allow this site"));
    }

    #[test]
    fn approvals_and_tool_results_render_in_the_conversation() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        let state = &mut app.view.state;
        state.history = vec![
            crate::ollama::Message::new("user", "Fix it"),
            crate::ollama::Message::new("assistant", "Checking."),
            crate::ollama::Message::new(
                "user",
                "Tool result c1 · Read src/a.py\nsrc/a.py · lines 1-2 of 2 · hash abc\n    1  def f(x):",
            ),
            crate::ollama::Message::new("assistant", ""),
        ];
        state.thread.tool_results.insert(2);
        state.thread.turns.insert(
            1,
            crate::state::Turn {
                tool: Some(crate::state::ToolNote {
                    summary: "Read src/a.py".into(),
                    status: "Done".into(),
                }),
                ..Default::default()
            },
        );
        let root = dir.path().to_owned();
        state.requests.push(crate::state::Pending {
            remote:None,
            chat_read_only: false,
            id: 4,
            chat: state.chat_id,
            project: Some(root.clone()),
            root: root.clone(),
            call: crate::tools::Envelope {
                v: 1,
                id: "c2".into(),
                call: crate::tools::Call::Patch {
                    path: "src/a.py".into(),
                    find: "x < 10".into(),
                    replace: "x <= 10".into(),
                    hash: "abc".into(),
                },
            },
            risk: crate::policy::Risk::Write,
            target: root.join("src/a.py"),
            model: "one".into(),
            trust: crate::policy::Trust::Standard,
            steps: 1,
            created: 0,
            approved: false,
            reason: String::new(),
            preview: String::new(),
        });
        app.page = Page::Chat;
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        // Tool steps start folded into one line.
        let folded = "Worked · 1 step · Read src/a.py";
        assert!(labels.contains_key(folded), "{:?}", labels.keys());
        assert!(!labels.contains_key("Read src/a.py · Done"));
        click(&mut app, &ctx, labels[folded]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(labels.contains_key("Worked · 1 step · Read src/a.py"));
        for text in [
            "Read src/a.py · Done",
            "Result · Read src/a.py",
            "Edit src/a.py",
            "- x < 10\n+ x <= 10",
            "Allow once",
            "Allow for this project",
            "Deny",
        ] {
            assert!(
                labels.contains_key(text),
                "missing {text:?} in {:?}",
                labels.keys()
            );
        }
        assert!(
            !labels.contains_key("    1  def f(x):"),
            "results start collapsed"
        );
        click(&mut app, &ctx, labels["Allow for this conversation"]);
        let sent: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert_eq!(sent, vec!["/approve 4 conversation".to_owned()]);
    }

    #[test]
    fn a_setup_request_reads_as_an_offer() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        let chat = app.view.state.chat_id;
        app.view.state.history = vec![
            crate::ollama::Message::new("user", "Make a docx"),
            crate::ollama::Message::new("assistant", ""),
        ];
        app.view.state.requests.push(crate::state::Pending {
            remote:None,
            chat_read_only: false,
            id: 2,
            chat,
            project: None,
            root: dir.path().into(),
            call: crate::tools::Envelope {
                v: 1,
                id: "s1".into(),
                call: crate::tools::Call::Setup {},
            },
            risk: crate::policy::Risk::Install,
            target: dir.path().into(),
            model: "small:7b".into(),
            trust: crate::policy::Trust::Restricted,
            steps: 0,
            created: 0,
            approved: false,
            reason: String::new(),
            preview: "Downloads Python".into(),
        });
        app.page = Page::Chat;
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        for text in ["Set up the sandbox", "Not now", "Downloads Python"] {
            assert!(
                labels.contains_key(text),
                "missing {text:?} in {:?}",
                labels.keys()
            );
        }
        click(&mut app, &ctx, labels["Set up the sandbox"]);
        let sent: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert_eq!(sent, vec!["/approve 2 once".to_owned()]);
    }

    #[test]
    fn sidebar_has_no_mode_switch_and_keeps_the_draft() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.page = Page::Chat;
        app.draft = "a moonlit lake".into();
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(!labels.contains_key("Make image"));
        assert!(!labels.contains_key("Cowork"));
        assert!(labels["Collapse sidebar"].y < labels["Work queue    0"].y);
        assert_eq!(app.draft, "a moonlit lake");
    }

    #[test]
    fn public_browser_opens_the_side_panel_while_loading() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (views, rx) = std::sync::mpsc::channel();
        let (_, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        let mut view = app.view.clone();
        view.browser_panel = Some(crate::execution::BrowserPanel {
            chat: view.state.chat_id,
            address: "https://example.com/".into(),
            activity: crate::execution::BrowserActivity::Opening,
        });
        views.send(view).unwrap();
        let labels = run(&mut app, &ctx, vec![]);
        assert!(app.reader.open);
        assert!(app.reader.tab == side_tabs::Tab::Browser);
        assert!(labels.contains_key("Spindle is opening this page…"));
    }

    #[test]
    fn turning_off_nerd_stats_reclaims_the_composer_footer() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (_, rx) = std::sync::mpsc::channel();
        let (_, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.draft = "Composer layout test".into();
        app.view.state.preferences.nerd_stats = true;
        run(&mut app, &ctx, vec![]);
        let with_stats = run(&mut app, &ctx, vec![]);
        assert!(with_stats.keys().any(|label| label.starts_with("Context ")));
        app.view.state.preferences.nerd_stats = false;
        run(&mut app, &ctx, vec![]);
        let compact = run(&mut app, &ctx, vec![]);
        assert!(!compact.keys().any(|label| label.starts_with("Context ")));
        assert!(compact["Composer layout test"].y > with_stats["Composer layout test"].y);
    }

    #[test]
    fn run_on_opens_a_panel_and_keeps_the_draft() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.draft = "Build a release".into();
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        let panel = click(&mut app, &ctx, labels["Run on · This device"]);
        for text in ["Default device", "Native builds and smoke tests", "Python scripts", "Installed tools", "Android APK builds"] {
            assert!(panel.contains_key(text), "missing execution control {text}");
        }
        click(&mut app, &ctx, panel["Back"]);
        assert!(app.execution_panel.is_none());
        assert_eq!(app.draft, "Build a release");
    }

    #[test]
    fn sandbox_changes_and_settings_show_their_state() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.view.state.thread.sandbox_changes =
            vec!["changed calc.py".into(), "new report.docx".into()];
        app.view.job = Some("Run the tests".into());
        app.page = Page::Chat;
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(
            labels.contains_key("2 changes in the sandbox copy, not yet in the project"),
            "{:?}",
            labels.keys()
        );
        assert!(labels.contains_key("Run the tests…"));
        click(&mut app, &ctx, labels["Review and apply…"]);
        let sent: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert_eq!(sent, vec!["/apply".to_owned()]);
        app.view.job = None;
        app.view.state.sandbox = Some(crate::sandbox::Report {
            backend: "bubblewrap (/usr/bin/bwrap)".into(),
            checks: vec![crate::sandbox::Check {
                name: "No network".into(),
                passed: false,
                detail: "allowed".into(),
            }],
            problem: "Some checks failed, so running code stays off.".into(),
            ..Default::default()
        });
        app.sandbox_tab = true;
        app.page = Page::Settings;
        app.view.state.preferences.browser_path = dir.path().join("missing.exe").display().to_string();
        let size = egui::vec2(1280., 1600.);
        run_at(&mut app, &ctx, vec![], size);
        let labels = run_at(&mut app, &ctx, vec![], size);
        if crate::browser::SUPPORTED {
            for text in ["Check browser", "Use auto-detection", "Download Chrome", "Download Edge", "Get Chromium"] {
                assert!(labels.contains_key(text), "missing {text:?} in {:?}", labels.keys());
            }
        }
        for text in [
            "Running code is off.",
            "No network",
            "✗",
            "Run the check",
            "Components",
            "spindle_docs",
            "Python packages",
        ] {
            assert!(
                labels.contains_key(text),
                "missing {text:?} in {:?}",
                labels.keys()
            );
        }
    }

    /// Text drawn past the right edge of a phone-sized screen, for each page and dialog.
    #[test]
    fn nothing_runs_off_a_phone_screen() {
        let phone = egui::vec2(412., 860.);
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        let overflow = |app: &mut Desktop| -> Vec<String> {
            run_at(app, &ctx, vec![], phone);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, phone)),
                ..Default::default()
            };
            let mut frame = eframe::Frame::_new_kittest();
            let output = ctx.run(input, |ctx| eframe::App::update(app, ctx, &mut frame));
            fn walk(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<String>) {
                match shape {
                    egui::Shape::Text(t) => {
                        let r = t.galley.rect.translate(t.pos.to_vec2());
                        // Cut off by the screen's edge (not by a scroll area inside it).
                        if r.intersects(clip) && clip.right() >= 411. && r.right() > 412. + 1. {
                            out.push(format!(
                                "{:?} from {:.0} to {:.0} (clip to {:.0})",
                                t.galley.text(),
                                r.left(),
                                r.right(),
                                clip.right()
                            ));
                        }
                    }
                    egui::Shape::Vec(v) => v.iter().for_each(|c| walk(c, clip, out)),
                    _ => {}
                }
            }
            let mut out = vec![];
            for c in &output.shapes {
                walk(&c.shape, c.clip_rect, &mut out);
            }
            out
        };
        let mut problems = vec![];
        let mut check = |name: &str, app: &mut Desktop| {
            for p in overflow(app) {
                problems.push(format!("{name}: {p}"));
            }
        };
        check("chat", &mut app);
        app.notice = "Sandbox status: this is a long notice with enough words to wrap across several lines. ".repeat(5);
        check("status notice", &mut app);
        assert!(run_at(&mut app, &ctx, vec![], phone).contains_key("Dismiss"));
        app.notice.clear();
        app.toast = None;
        app.page = Page::Settings;
        check("providers", &mut app);
        app.provider_setup = super::super::settings::ProviderSetup::Choose;
        check("provider chooser", &mut app);
        app.provider_setup = super::super::settings::ProviderSetup::Configure("Ollama");
        check("provider setup form", &mut app);
        app.provider_setup = Default::default();
        app.models_tab = true;
        check("models", &mut app);
        app.models_tab = false;
        app.memory_tab = true;
        check("memory", &mut app);
        app.memory_tab = false;
        app.sandbox_tab = true;
        check("sandbox", &mut app);
        app.sandbox_tab = false;
        app.preferences_tab = true;
        check("preferences", &mut app);
        app.preferences_tab = false;
        app.sync_tab = true;
        check("sync", &mut app);
        app.sync_tab = false;
        app.page = Page::Chat;
        app.edit_thread(app.view.state.chat_id);
        check("conversation settings", &mut app);
        let fields = run_at(&mut app, &ctx, vec![], egui::vec2(360., 450.));
        for label in ["Name", "Model", "Project", "Section", "Save"] {
            assert!(fields.contains_key(label), "conversation settings missing {label}");
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// A phone: one column, the sidebar as a drawer that closes when you pick something.
    #[test]
    fn a_phone_sized_window_uses_a_drawer() {
        let phone = egui::vec2(412., 860.);
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        let tap = |app: &mut Desktop, pos: Pos2| {
            for pressed in [true, false] {
                run_at(
                    app,
                    &ctx,
                    vec![
                        Event::PointerMoved(pos),
                        Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                    phone,
                );
            }
            run_at(app, &ctx, vec![], phone)
        };
        run_at(&mut app, &ctx, vec![], phone);
        let labels = run_at(&mut app, &ctx, vec![], phone);
        assert!(
            !labels.contains_key("Settings"),
            "no sidebar: {:?}",
            labels.keys()
        );
        // The menu button at the top left opens the drawer.
        let labels = tap(&mut app, Pos2::new(28., 25.));
        assert!(app.drawer, "{:?}", labels.keys());
        assert!(labels.contains_key("×") && labels.contains_key("Settings"));
        // Choosing a page closes it.
        let labels = tap(&mut app, labels["Settings"]);
        assert!(!app.drawer);
        assert!(app.page == Page::Settings, "{:?}", labels.keys());
        // Every settings tab draws at phone width, Sync included.
        app.sync_tab = true;
        run_at(&mut app, &ctx, vec![], phone);
        let labels = run_at(&mut app, &ctx, vec![], phone);
        assert!(labels.contains_key("Start syncing"), "{:?}", labels.keys());
        // Back from Preferences returns to the chat and keeps its draft.
        let chat = app.view.state.chat_id;
        app.draft = "draft before settings".into();
        tap(&mut app, labels["Preferences"]);
        assert!(app.preferences_tab);
        tap(&mut app, Pos2::new(23., 25.));
        assert_eq!(app.page, Page::Chat);
        assert_eq!(app.view.state.chat_id, chat);
        assert_eq!(app.draft, "draft before settings");
        // The side panel covers the screen and × brings the chat back.
        app.reader.open = true;
        run_at(&mut app, &ctx, vec![], phone);
        let labels = run_at(&mut app, &ctx, vec![], phone);
        assert!(labels["Viewer"].x < labels["Files"].x);
        assert!(labels["Browser"].x < labels["Plan"].x && labels["Plan"].x < labels["×"].x);
        let labels = tap(&mut app, labels["Plan"]);
        assert!(app.reader.tab == side_tabs::Tab::Plan && labels.contains_key("Add a step"));
        let labels = tap(&mut app, labels["Revise plan…"]);
        assert!(labels.contains_key("Apply change") && labels.contains_key("Replace the current objective"));
        let labels = tap(&mut app, labels["Back"]);
        let close = labels["×"];
        assert!(close.x > 300., "{close:?}");
        tap(&mut app, close);
        assert!(!app.reader.open);
    }

    #[test]
    fn mobile_settings_back_restores_the_origin_even_from_a_detail_panel() {
        let phone = egui::vec2(320., 720.);
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (_, rx) = std::sync::mpsc::channel();
        let (_, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.page = Page::Project;
        app.open_settings();
        app.open_settings();
        app.details = true;
        run_at(&mut app, &ctx, vec![], phone);
        app.go_back(&ctx);
        run_at(&mut app, &ctx, vec![], phone);
        assert_eq!(app.page, Page::Project);
        assert!(!app.details);
        app.go_back(&ctx);
        assert_eq!(app.page, Page::Chat);
    }

    #[test]
    fn long_council_names_keep_remove_and_add_visible_on_a_phone() {
        let phone = egui::vec2(320., 3200.);
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_, rx) = std::sync::mpsc::channel();
        let (_, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.view.state.preferences.text_size = 22;
        app.open_settings();
        app.preferences_tab = true;
        let members = [
            "qwen3.5:9b",
            "Maternion/spark-x2.6:4b-Q8_0-some-very-long-unbroken-model-tag",
        ];
        for (kept, removed) in [(members[0], members[1]), (members[1], members[0])] {
            app.view.state.council = vec![kept.into(), removed.into()];
            run_at(&mut app, &ctx, vec![], phone);
            let labels = run_at(&mut app, &ctx, vec![], phone);
            let pos = labels["Remove"];
            assert!(pos.x > 230. && pos.x < 288., "{pos:?}");
            assert!(labels.keys().any(|label| label.ends_with("Add a model…")));
            for pressed in [true, false] {
                run_at(&mut app, &ctx, vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() },
                ], phone);
            }
            let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
            assert!(sent.contains(&format!("/council set {kept}")), "{sent:?}");
        }
    }

    /// A project from another device: Spindle asks where projects go, and the folder can be
    /// picked by browsing (no path typing needed) and made on the spot.
    #[test]
    fn the_projects_folder_is_asked_for_and_picked_by_browsing() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("Docs");
        std::fs::create_dir_all(docs.join("Work")).unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        let space = crate::sync::space::Space::create("Phone", None).unwrap();
        let mut sync = crate::sync::adapter::SyncState::new(space.id.clone());
        sync.remote_projects
            .insert("p-1".into(), serde_json::json!({"name": "VRT reports"}));
        app.view.state.space = Some(sync);
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(
            labels.contains_key("Where should projects go?")
                && labels.contains_key("Waiting for a folder: VRT reports"),
            "{:?}",
            labels.keys()
        );
        // Browse (Linux and phones use Spindle's own browser) from the Docs folder.
        app.sync_form.browse = Some(docs.clone());
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(
            labels.contains_key("Choose a folder"),
            "{:?}",
            labels.keys()
        );
        click(&mut app, &ctx, labels["📁  Work"]);
        let labels = run(&mut app, &ctx, vec![]);
        assert_eq!(
            app.sync_form.browse.as_deref(),
            Some(docs.join("Work").as_path())
        );
        // Make a folder there and use it. Typed once, it arrives once (it was doubled when
        // the browser was drawn from two places), with the Sync page showing too.
        app.page = Page::Settings;
        app.sync_tab = true;
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        click(&mut app, &ctx, labels["New folder name"]);
        run(&mut app, &ctx, vec![Event::Text("Spindle".into())]);
        assert_eq!(app.sync_form.new_folder, "Spindle");
        let labels = run(&mut app, &ctx, vec![]);
        click(&mut app, &ctx, labels["New folder"]);
        let target = docs.join("Work").join("Spindle");
        let _ = &labels;
        assert!(target.is_dir());
        let labels = run(&mut app, &ctx, vec![]);
        click(&mut app, &ctx, labels["Use this folder"]);
        let sent: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(
            sent.contains(&format!("/sync projects-folder {}", target.display())),
            "{sent:?}"
        );
        assert!(app.sync_form.browse.is_none());
    }

    #[test]
    fn sync_settings_header_and_branches_show_their_state() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.sync_tab = true;
        app.page = Page::Settings;
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(labels.contains_key("Start syncing"), "{:?}", labels.keys());
        assert!(labels.contains_key("Join"));

        // Set up, with a server, and a chat with a branch from another device.
        let space = crate::sync::space::Space::create("Desktop", None).unwrap();
        let mut sync = crate::sync::adapter::SyncState::new(space.id.clone());
        sync.links.push(crate::sync::adapter::Link {
            place: crate::sync::adapter::Place::Hosted {
                url: "https://sync.example".into(),
            },
            progress: Default::default(),
        });
        sync.status.last_ok = Some(crate::automation::now() - 120);
        let chat = app.view.state.chat_id;
        sync.branches.insert(
            chat,
            vec![crate::sync::adapter::BranchInfo {
                id: "b1".into(),
                device: "x".into(),
                device_name: "Laptop".into(),
                fork: 1,
                new_from: 1,
                info: Default::default(),
                messages: vec![
                    crate::ollama::Message::new("user", "hi"),
                    crate::ollama::Message::new("user", "from laptop"),
                ],
            }],
        );
        app.view.state.space = Some(sync);
        app.view.state.history = vec![crate::ollama::Message::new("user", "hi")];
        let labels = run(&mut app, &ctx, vec![]);
        for text in [
            "Add a device",
            "Network and servers",
            "Projects from other devices",
            "Sync now",
            "Synced 2 min ago",
        ] {
            assert!(
                labels.contains_key(text),
                "missing {text:?} in {:?}",
                labels.keys()
            );
        }
        app.page = Page::Chat;
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(
            labels.contains_key("Branch from Laptop"),
            "{:?}",
            labels.keys()
        );
        // No other devices on this space's list in the test: the sidebar footer says so.
        assert!(
            labels.contains_key("⟳ Local"),
            "{:?}",
            labels.keys()
        );
        assert!((labels["⟳ Local"].y - labels["Settings"].y).abs() < 4.);
        click(&mut app, &ctx, labels["⟳ Local"]);
        assert_eq!(commands.try_recv().unwrap(), "/sync now");
        click(&mut app, &ctx, labels["Merge"]);
        assert_eq!(commands.try_recv().unwrap(), format!("/branch merge {chat} b1"));
        click(&mut app, &ctx, labels["Make main"]);
        let sent: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert_eq!(sent, vec![format!("/branch main {chat} b1")]);
    }

    #[test]
    fn conversation_text_can_be_selected_and_copied() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        app.view.state.history = vec![
            crate::ollama::Message::new("user", "Why is the sky blue?"),
            crate::ollama::Message::new("assistant", "Sunlight scatters off the air."),
        ];
        app.page = Page::Chat;
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        let at = labels["Sunlight scatters off the air."];
        // Start and finish inside the text, including with narrower system fonts.
        let from = labels["Why is the sky blue?"];
        let to = at + egui::vec2(52., 0.);
        let press = |pos, pressed| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(&mut app, &ctx, vec![Event::PointerMoved(from)]);
        run(&mut app, &ctx, vec![press(from, true)]);
        run(&mut app, &ctx, vec![Event::PointerMoved(at)]);
        run(&mut app, &ctx, vec![Event::PointerMoved(to)]);
        run(&mut app, &ctx, vec![press(to, false)]);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(1280., 1200.),
            )),
            events: vec![Event::Copy],
            ..Default::default()
        };
        let mut frame = eframe::Frame::_new_kittest();
        let out = ctx.run(input, |ctx| eframe::App::update(&mut app, ctx, &mut frame));
        let copied: Vec<String> = out
            .platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(
            copied.iter().any(|t| t.contains("blue?\nSunlight scatters") && !t.contains("Why") && !t.contains("air.")),
            "copied {copied:?}"
        );
    }

    #[test]
    fn a_file_made_by_a_script_shows_as_a_card() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (tx, _commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, root.clone(), tx, rx, preview, Accent::Blue);
        let chat = app.view.state.chat_id;
        let folder = crate::artifacts::folder(&root, chat).unwrap();
        std::fs::write(folder.join("why.docx"), vec![0u8; 3745]).unwrap();
        app.view.state.history = vec![
            crate::ollama::Message::new("user", "Make it"),
            crate::ollama::Message::new("assistant", "Done.\n\nSaved file: why.docx"),
        ];
        app.view.state.thread.turns.insert(
            1,
            crate::state::Turn {
                files: vec!["why.docx".into()],
                ..Default::default()
            },
        );
        app.page = Page::Chat;
        run(&mut app, &ctx, vec![]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let labels = loop {
            let labels = run(&mut app, &ctx, vec![]);
            if labels.contains_key("DOCX · 3.7 KB") || std::time::Instant::now() >= deadline {
                break labels;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(labels.contains_key("why.docx"), "{:?}", labels.keys());
        assert!(
            !labels.keys().any(|k| k.contains("Saved file:")),
            "{:?}",
            labels.keys()
        );
        assert!(labels.contains_key("DOCX · 3.7 KB"), "{:?}", labels.keys());
    }

    #[test]
    fn notes_open_the_memory_and_the_plan() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        let state = &mut app.view.state;
        state.history = vec![
            crate::ollama::Message::new("user", "Plan the move"),
            crate::ollama::Message::new("assistant", "Done."),
        ];
        crate::memory::save(state, "Prefers short answers", None, 0);
        state.thread.steps = vec![Step {
            text: "Port templates".into(),
            done: false,
        }];
        state.thread.turns.insert(
            1,
            crate::state::Turn {
                remembered: vec![1],
                plan: Some(Update {
                    added: 1,
                    changed: vec![0],
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        state.thread.plan_notes.insert(
            2,
            Update {
                completed: 1,
                ..Default::default()
            },
        );
        app.page = Page::Chat;
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(
            labels.contains_key("You updated the plan · 1 done"),
            "{:?}",
            labels.keys()
        );
        let labels = click(&mut app, &ctx, labels["Plan updated · 1 added"]);
        assert!(app.reader.open && app.reader.tab == side_tabs::Tab::Plan && app.plan.highlight == vec![0]);
        assert!(labels.contains_key("Port templates"));
        click(
            &mut app,
            &ctx,
            labels["Memory updated · Prefers short answers"],
        );
        assert!(app.page == Page::Settings && app.memory_tab);
        assert_eq!(app.memory.highlight, vec![1]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(labels.contains_key("Prefers short answers") && labels.contains_key("Personal"));
        // Personal and project memory each have a switch.
        let labels = click(&mut app, &ctx, labels["Project memory"]);
        let sent: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(
            sent.iter()
                .any(|c| c.starts_with("/preferences") && c.contains("\"project_memory\":false")),
            "{sent:?}"
        );
        assert!(labels.contains_key("Personal memory"));
        // A project's own page of memories, with its switch.
        let plans = dir.path().join("plans");
        app.view.state.memories.push(crate::memory::Memory {
            id: 9,
            text: "Uses Zola".into(),
            project: Some(plans.clone()),
            saved: 0,
        });
        app.show_memories(Some(plans.clone()));
        run(&mut app, &ctx, vec![]);
        let labels = run(&mut app, &ctx, vec![]);
        assert!(
            labels.contains_key("Uses Zola") && !labels.contains_key("Prefers short answers"),
            "{:?}",
            labels.keys()
        );
        click(&mut app, &ctx, labels["Use memory in this project's chats"]);
        let sent: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(
            sent.iter()
                .any(|c| c.starts_with("/memory-project") && c.ends_with(",false]")),
            "{sent:?}"
        );
    }
}
