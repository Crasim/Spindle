# Spindle agent workflows

Design addendum for integration with the command catalog, architecture, implementation plan, and evidence log. Command names below describe intents. Bind them to the existing commands and aliases when reconciling the catalog.

## From command to verified result

Suppose the user asks for a plan to rename a configuration key. The planner reads the loader, identifies compatibility requirements, and proposes edits and a test. The user accepts revision 2. A research subagent can now inspect callers while the lead agent edits the loader. If a test needs network access, the harness shows the specific request before starting the test. If the user declines, the lead can still run local checks and report which check remains outstanding.

| User intent | Harness behavior |
| --- | --- |
| Plan a task | Create a draft with a goal, steps, dependencies, and a verification condition for each deliverable. Permit inspection and private plan storage. Restrict workspace mutations and external side effects throughout the planning subtree. |
| Revise or accept | Store a new numbered revision on edit. Acceptance names the revision and starts its ready steps. Reject a stale acceptance and display the current revision. A material scope change returns the affected work to review. |
| Delegate | Bind a bounded assignment to a step, giving the child the relevant inputs, allowed tools, writable scope, and completion condition. Return a child ID immediately. |
| Inspect status | Show the current revision, ready/running/blocked steps, active children, pending approvals, and available evidence from the same run record. |
| Stop or resume | Stop new dispatch, cancel descendants, and invalidate outstanding approval requests. On resume, reconcile interrupted work against observed effects before retrying. |

Slash commands, UI controls, and agent tool calls share typed handlers. The dispatcher validates arguments and resolves the run, revision, and actor before invoking a handler. Unknown commands and ambiguous aliases return a usage error. A command that expands into several tool calls passes each call through the execution policy.

## Subagent ownership

The parent owns integration and verification. Each child gets a fresh conversation with a brief and explicit artifact references. Its effective capabilities are the intersection of parent, role, run mode, and assignment permissions. Start with one delegation level and a configurable concurrency cap. Queue excess jobs, reserve a share of the run budget per active child, and return unused budget when it exits.

Parallelize independent reads. Serialize writers with overlapping target paths or use isolated worktrees and let the parent integrate the diffs. When a child discovers it needs broader access, it reports the blocker to the parent. Its result records status, changed files, checks and their outcomes, and unresolved issues. The parent verifies the combined changes before closing the step. Record a child's terminal result before notifying the parent, and deduplicate notifications by child run ID.

## Runtime permissions and approvals

Evaluate each proposed action immediately before execution. Resolve the actual tool, normalized arguments, working directory, affected resources, and caller. Apply hard environment restrictions first, then role and task limits, then matching grants. Use three outcomes.

| Outcome | Effect |
| --- | --- |
| Allow | Execute within the granted scope and record the result. |
| Ask | Persist a pending request and pause that action. Show the requesting agent and plan step, exact operation and target, reason, and expected effect. Independent work can continue. |
| Deny | Return the policy reason and mark the dependent step blocked if it has no permitted alternative. |

Offer **allow once**, **deny**, and, when the scope is safely expressible, **allow matching actions for this run**. Display the exact match scope before saving a grant. Bind one-time decisions to a unique invocation and its normalized arguments. Recheck current policy and resolved targets at dispatch. A changed command, destination, or resource requires a fresh decision. Plan acceptance only schedules work. It creates no tool grants.

For shell tools, enforce filesystem and network limits in the executor. A command-prefix match alone cannot describe the effects of a pipeline, redirection, or nested interpreter. Where effects cannot be bounded by a reusable rule, present the entire invocation for a one-time decision within the permitted sandbox. Closing a prompt, losing the client, or reaching a timeout leaves the operation unexecuted.

## Implementation order and recovery

1. Route commands through the shared dispatcher. Persist run, step, revision, and invocation IDs before making work visible to the scheduler.
2. Implement planning restrictions and the policy gate at the executor boundary, including direct calls and child calls. Journal requests and decisions. Consume a one-time grant and claim its invocation atomically so two clients cannot dispatch it twice.
3. Add the scheduler, child lifecycle, cancellation propagation, and result delivery. On restart, restore pending prompts and collect persisted child results. An invocation interrupted after dispatch has an unknown outcome until reconciled. Automatically retry only operations known to be safe to repeat.
4. Connect status, approval prompts, and final reporting to the journal. Redact secrets in previews and logs while retaining the operation identity, decision scope, and verification outcome.

## Acceptance scenarios

| Exercise | Observable result |
| --- | --- |
| Ask a planner and its research child to edit a project file | Both calls are blocked before a write. Saving a plan revision succeeds. |
| Accept revision 1 after revision 2 appears | No work starts. The user sees revision 2. |
| Accept a plan, then reach a tool action requiring approval | The action waits. Denial blocks that action and leaves independent work runnable. |
| Approve one destination, then change the destination before dispatch | The old decision cannot authorize the changed invocation. |
| Click allow twice from different clients | One invocation is claimed and dispatched. |
| Cancel while a child awaits approval, then submit the old approval | The request is stale. The child stays cancelled. |
| Restart after a child finishes but before the parent sees its result | The parent receives one logical result and can inspect the evidence. |
| Lose the executor after an external action starts | Status shows an uncertain outcome and recovery checks for the effect before retrying. |

## Reference decisions

OpenCode documents separate agent roles, a planning role with narrowly scoped writes, and allow/ask/deny permissions. Spindle can use that separation to close a common gap where the UI says “planning” but a shell or delegated tool can still change the workspace. [OpenCode agents](https://opencode.ai/docs/agents/) · [OpenCode permissions](https://opencode.ai/docs/permissions/)

Hermes documents background delegation handles and persistence of completion events before delivery. The proposed child ID and durable result record address the same failure case when the parent reconnects between job completion and notification. [Hermes delegation](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/delegation.md)
