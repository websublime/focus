//! A guard's recovery operations (`aidlc-guard-operation.ts`): the ways on
//! a refusing guard puts to the person, as domain operations rather than
//! shell programs. An operation renders as the engine command the conductor
//! runs for it, in the install's own spelling (`focus engine <route> …`,
//! or a source tree's `bun <harness>/tools/focus-<route>.ts …`), and a
//! command typed back is matched against that rendering exactly: wrappers,
//! redirections, extra flags and trailing commands are never part of a
//! remedy. Rendering an operation authenticates no selection: the owning
//! commands keep their own checks, and no recovery operation approves a
//! plan, records a verdict or invents feedback.
//!
//! Held to upstream's answers by the oracle corpus (`guard_operation`).

use focus_js::re;
use serde_json::Value;

use crate::fences::Fence;

/// What a refusing guard offers the person as a way on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardRecoveryOperation {
    RestartStage {
        stage: String,
    },
    AbortBolt {
        unit: String,
        slug: String,
    },
    /// The switchable set is the source of truth for fence recovery.
    LowerFence {
        fence: Fence,
    },
    /// Records the Unit's missing `UNIT_COMPLETED` receipt from artifacts
    /// already on disk; the state tool checks them and the open ask before it
    /// writes.
    RecordUnitCompletion {
        stage: String,
        unit: String,
    },
    /// Starts one Unit's step of a solo unit-major walk again. Every other
    /// Unit keeps its finished work.
    ReopenUnit {
        stage: String,
        unit: String,
    },
    /// Sets this work's reviews to advisory, so the reviewer's open findings
    /// go to the approval gate for the person instead of another review pass.
    ReviewAdvisory,
}

/// One engine command: `engine <route> <args>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInvocation {
    pub route: String,
    pub args: Vec<String>,
}

/// An operation's engine command, with its source-tool spelling when that
/// differs from the native route (the fence switch runs through `config`
/// natively and through `utility config-change` in a source tree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardOperationInvocation {
    pub route: &'static str,
    pub args: Vec<String>,
    pub source: Option<EngineInvocation>,
}

impl GuardOperationInvocation {
    fn native(&self) -> EngineInvocation {
        EngineInvocation {
            route: self.route.to_owned(),
            args: self.args.clone(),
        }
    }
}

/// `source` renders `bun <harness>/tools/focus-<route>.ts`, `native`
/// `focus engine <route>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Source,
    Native,
}

/// The shell a rendered command is quoted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderShell {
    Posix,
    PowerShell,
}

/// The native spelling of the engine command, as this install runs it.
pub const NATIVE_INVOCATION: &str = "focus";

fn is_identifier(value: &str) -> bool {
    re::test(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,255}$", "", value)
}

fn identifier(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| is_identifier(text))
}

/// `isGuardRecoveryOperation`: a JSON value that is exactly one operation,
/// with no field beyond its own.
#[must_use]
pub fn parse_operation(value: &Value) -> Option<GuardRecoveryOperation> {
    let operation = value.as_object()?;
    let field = |name: &str| identifier(operation.get(name)).map(str::to_owned);
    match operation.get("kind").and_then(Value::as_str)? {
        "restart-stage" if operation.len() == 2 => Some(GuardRecoveryOperation::RestartStage {
            stage: field("stage")?,
        }),
        "abort-bolt" if operation.len() == 3 => Some(GuardRecoveryOperation::AbortBolt {
            unit: field("unit")?,
            slug: field("slug")?,
        }),
        "lower-fence" if operation.len() == 2 => {
            let fence = Fence::parse(operation.get("fence")?.as_str()?)?;
            fence
                .is_switchable()
                .then_some(GuardRecoveryOperation::LowerFence { fence })
        }
        "review-advisory" if operation.len() == 1 => Some(GuardRecoveryOperation::ReviewAdvisory),
        "record-unit-completion" if operation.len() == 3 => {
            Some(GuardRecoveryOperation::RecordUnitCompletion {
                stage: field("stage")?,
                unit: field("unit")?,
            })
        }
        "reopen-unit" if operation.len() == 3 => Some(GuardRecoveryOperation::ReopenUnit {
            stage: field("stage")?,
            unit: field("unit")?,
        }),
        _ => None,
    }
}

/// `guardOperationInvocation`: the engine command an operation runs as.
#[must_use]
pub fn invocation(operation: &GuardRecoveryOperation) -> GuardOperationInvocation {
    let strings = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
    match operation {
        GuardRecoveryOperation::RestartStage { stage } => GuardOperationInvocation {
            route: "orchestrate",
            args: strings(&["next", "--stage", stage]),
            source: None,
        },
        GuardRecoveryOperation::AbortBolt { unit, slug } => GuardOperationInvocation {
            route: "bolt",
            args: strings(&[
                "abort",
                "--name",
                unit,
                "--slug",
                slug,
                "--reason",
                "stale review recovery exhausted",
                "--discard",
            ]),
            source: None,
        },
        GuardRecoveryOperation::LowerFence { fence } => {
            // The `guard.` prefix is the config-key spelling the fences own.
            let key = fence.config_key();
            GuardOperationInvocation {
                route: "config",
                args: strings(&["set", &key, "off"]),
                source: Some(EngineInvocation {
                    route: "utility".to_owned(),
                    args: strings(&["config-change", &format!("--{key}"), "off"]),
                }),
            }
        }
        GuardRecoveryOperation::RecordUnitCompletion { stage, unit } => GuardOperationInvocation {
            route: "state",
            args: strings(&["unit", "complete", "--stage", stage, "--unit", unit]),
            source: None,
        },
        GuardRecoveryOperation::ReopenUnit { stage, unit } => GuardOperationInvocation {
            route: "jump",
            args: strings(&["reopen", "--target", stage, "--units", unit]),
            source: None,
        },
        GuardRecoveryOperation::ReviewAdvisory => GuardOperationInvocation {
            route: "config",
            args: strings(&["set", "review", "advisory"]),
            source: Some(EngineInvocation {
                route: "utility".to_owned(),
                args: strings(&["config-change", "--review", "advisory"]),
            }),
        },
    }
}

/// `quoteCommandArgument` (`aidlc-runtime-paths.ts`): a word as a shell
/// reads it back. Control characters become `?`; a plain word stands as it
/// is; anything else is single-quoted for the shell named.
#[must_use]
pub fn quote_command_argument(value: &str, shell: RenderShell) -> String {
    let value: String = value
        .chars()
        .map(|c| {
            if ('\u{0}'..='\u{1f}').contains(&c) || c == '\u{7f}' {
                '?'
            } else {
                c
            }
        })
        .collect();
    if re::test(r"^[A-Za-z0-9_./:@%+=,-]+$", "", &value) {
        return value;
    }
    match shell {
        RenderShell::PowerShell => format!("'{}'", value.replace('\'', "''")),
        RenderShell::Posix => format!("'{}'", value.replace('\'', "'\"'\"'")),
    }
}

/// `renderEngineInvocation`: the command line for an engine invocation.
///
/// # Errors
///
/// The harness directory is not a dot-directory name.
pub fn render_engine_invocation(
    invocation: &EngineInvocation,
    mode: RenderMode,
    harness_dir: &str,
    shell: RenderShell,
) -> Result<String, String> {
    if !re::test(r"^\.[A-Za-z0-9_.-]+$", "", harness_dir) {
        return Err("Invalid recovery harness directory".to_owned());
    }
    let prefix = match mode {
        RenderMode::Native => format!(
            "{NATIVE_INVOCATION} engine {}",
            quote_command_argument(&invocation.route, shell)
        ),
        RenderMode::Source => format!(
            "bun {}",
            quote_command_argument(
                &format!(
                    "{harness_dir}/tools/{NATIVE_INVOCATION}-{}.ts",
                    invocation.route
                ),
                shell
            )
        ),
    };
    let args: Vec<String> = invocation
        .args
        .iter()
        .map(|arg| quote_command_argument(arg, shell))
        .collect();
    Ok(format!("{prefix} {}", args.join(" ")))
}

/// `renderGuardOperation`: the operation's own command, in its source
/// spelling when it has one and the mode is `source`.
///
/// # Errors
///
/// As [`render_engine_invocation`].
pub fn render(
    operation: &GuardRecoveryOperation,
    mode: RenderMode,
    harness_dir: &str,
    shell: RenderShell,
) -> Result<String, String> {
    let invocation = invocation(operation);
    let spelled = match (mode, &invocation.source) {
        (RenderMode::Source, Some(source)) => source.clone(),
        _ => invocation.native(),
    };
    render_engine_invocation(&spelled, mode, harness_dir, shell)
}

/// `guardOperationMatchesCommand`: the command is exactly the operation's
/// rendering, natively for either shell, or in its source spelling for the
/// harness directory the command itself names.
#[must_use]
pub fn matches_command(operation: &GuardRecoveryOperation, command: &str) -> bool {
    let source = re::find(
        r"^bun (\.[A-Za-z0-9_.-]+)/tools/focus-[A-Za-z0-9_-]+\.ts ",
        "",
        command,
    )
    .and_then(|found| re::group(command, &found, 1).map(str::to_owned));
    for shell in [RenderShell::Posix, RenderShell::PowerShell] {
        // The native rendering names no harness directory; any valid one
        // serves.
        if render(operation, RenderMode::Native, ".claude", shell).as_deref() == Ok(command) {
            return true;
        }
        if let Some(named) = &source
            && render(operation, RenderMode::Source, named, shell).as_deref() == Ok(command)
        {
            return true;
        }
    }
    false
}

/// `guardOperationMatchesRemedy`: the operation carries out the named
/// remedy for this stage (and Unit).
#[must_use]
pub fn matches_remedy(
    operation: &GuardRecoveryOperation,
    remedy: &str,
    stage: &str,
    unit: Option<&str>,
) -> bool {
    match operation {
        GuardRecoveryOperation::RestartStage { stage: own } => {
            ["restart-stage", "redo-jump", "restore-or-jump"].contains(&remedy) && own == stage
        }
        GuardRecoveryOperation::AbortBolt { unit: own, .. } => {
            remedy == "abort-bolt" && Some(own.as_str()) == unit
        }
        GuardRecoveryOperation::LowerFence { .. } => remedy == "lower-fence",
        GuardRecoveryOperation::RecordUnitCompletion {
            stage: own_stage,
            unit: own_unit,
        } => {
            remedy == "record-unit-completion"
                && own_stage == stage
                && Some(own_unit.as_str()) == unit
        }
        GuardRecoveryOperation::ReopenUnit {
            stage: own_stage,
            unit: own_unit,
        } => remedy == "reopen-unit-step" && own_stage == stage && Some(own_unit.as_str()) == unit,
        GuardRecoveryOperation::ReviewAdvisory => remedy == "review-advisory-gate",
    }
}

/// `sameGuardOperation`: two recorded operations are the same one. An
/// absent side equals only an absent side; a malformed side equals nothing.
#[must_use]
pub fn same(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => match (parse_operation(left), parse_operation(right)) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        },
        _ => false,
    }
}

/// Which way a stage restart moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDirection {
    Redo,
    Backward,
}

impl RestartDirection {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Redo => "redo",
            Self::Backward => "backward",
        }
    }
}

/// The second command `next --stage` returns to perform the reset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardRestartContinuation {
    pub stage: String,
    pub direction: RestartDirection,
    pub scope: String,
}

/// `parseGuardRestartContinuationCommand`: the complete native
/// `focus engine jump execute --target <stage> --direction <redo|backward>
/// --scope <scope>` (or the source tree's `bun <harness>/tools/focus-jump.ts
/// …` for the harness directory passed), and nothing else.
#[must_use]
pub fn parse_guard_restart_continuation_command(
    command: &str,
    harness_dir: Option<&str>,
) -> Option<GuardRestartContinuation> {
    let mut words = command.split(' ');
    let executable = words.next().unwrap_or("");
    let words: Vec<&str> = words.collect();
    let args: &[&str] = if (executable == NATIVE_INVOCATION
        || executable == format!("{NATIVE_INVOCATION}.exe"))
        && words.first() == Some(&"engine")
        && words.get(1) == Some(&"jump")
    {
        &words[2..]
    } else if executable == "bun"
        && let Some(harness) = harness_dir
        && words.first().copied()
            == Some(format!("{harness}/tools/{NATIVE_INVOCATION}-jump.ts").as_str())
    {
        &words[1..]
    } else {
        return None;
    };
    if args.len() != 7
        || args[0] != "execute"
        || args[1] != "--target"
        || !is_identifier(args[2])
        || args[3] != "--direction"
        || !(args[4] == "redo" || args[4] == "backward")
        || args[5] != "--scope"
        || !is_identifier(args[6])
    {
        return None;
    }
    Some(GuardRestartContinuation {
        stage: args[2].to_owned(),
        direction: if args[4] == "redo" {
            RestartDirection::Redo
        } else {
            RestartDirection::Backward
        },
        scope: args[6].to_owned(),
    })
}

/// `isGuardRecoveryEngineInvocation`: the fully specified native abort or
/// fence switch (natively or through the source tool), and nothing else.
#[must_use]
pub fn is_guard_recovery_engine_invocation(args: &[String]) -> bool {
    if args.first().map(String::as_str) != Some("engine") {
        return false;
    }
    let second = args.get(1).map(String::as_str);
    let switchable = |name: &str| Fence::parse(name).filter(|fence| fence.is_switchable());
    // The abort's own length guards the two words read here; the exact
    // comparison below is what refuses a longer or shorter line.
    let operation = if second == Some("bolt") && args.len() == 10 {
        GuardRecoveryOperation::AbortBolt {
            unit: args[4].clone(),
            slug: args[6].clone(),
        }
    } else if second == Some("config")
        && args.len() == 5
        && let Some(fence) = args[3].strip_prefix("guard.")
    {
        let Some(fence) = switchable(fence) else {
            return false;
        };
        GuardRecoveryOperation::LowerFence { fence }
    } else if second == Some("utility")
        && args.len() == 5
        && args[2] == "config-change"
        && let Some(fence) = args[3].strip_prefix("--guard.")
    {
        let Some(fence) = switchable(fence) else {
            return false;
        };
        GuardRecoveryOperation::LowerFence { fence }
    } else {
        return false;
    };
    if let GuardRecoveryOperation::AbortBolt { unit, slug } = &operation
        && !(is_identifier(unit) && is_identifier(slug))
    {
        return false;
    }
    matches_engine_args(&operation, args)
}

/// `guardOperationMatchesEngineArgs`: the operation's own argv, in its
/// native route or its source-tool spelling, as `engine <route> <args>`.
/// Exact: no extra flag, value or trailing word.
#[must_use]
pub fn matches_engine_args(operation: &GuardRecoveryOperation, args: &[String]) -> bool {
    let invocation = invocation(operation);
    let mut spellings = vec![invocation.native()];
    if let Some(source) = invocation.source {
        spellings.push(source);
    }
    spellings.iter().any(|spelling| {
        let mut expected = vec!["engine".to_owned(), spelling.route.clone()];
        expected.extend(spelling.args.iter().cloned());
        expected == args
    })
}

/// What the person reads for a remedy: the code the guard refused with, the
/// stage's plain name (and with its Unit), the fence and whether a
/// stage-wide reset reaches every Unit.
#[derive(Debug, Clone, Default)]
pub struct RemedyWordingContext<'a> {
    pub code: &'a str,
    pub stage: &'a str,
    pub target: &'a str,
    pub unit: Option<&'a str>,
    pub fence: Option<Fence>,
    pub source_unbindable: bool,
    pub every_unit: bool,
}

/// Every remedy op the engine may offer (`GUARD_REMEDY_WORDING`'s keys), in
/// upstream's order; [`remedy_wording`] says which have words for the
/// person.
pub const REMEDY_OPS: [&str; 22] = [
    "present-approval-gate",
    "request-review",
    "start-recovery-review",
    "apply-repairs-then-request",
    "record-verdict",
    "retry-pending",
    "request-changes",
    "finish-revision",
    "redo-jump",
    "restore-or-jump",
    "restart-stage",
    "redo-unit-step",
    "reopen-unit-step",
    "review-advisory-gate",
    "change-scope",
    "restore-scope",
    "abort-bolt",
    "record-unit-completion",
    "repair-source-boundary",
    "reconfirm-summary",
    "unset-unattended",
    "lower-fence",
];

/// A check as the person knows it ("review freeze", "reviewer read scope").
fn check_words(fence: Fence) -> String {
    fence
        .name()
        .replace("reviewer-scope", "reviewer read scope")
        .replace('-', " ")
}

/// What a stage-wide reset in a solo unit-major walk throws away.
fn reset_cost(context: &RemedyWordingContext<'_>, reset: &str) -> String {
    if !context.every_unit {
        return String::new();
    }
    if reset == "jump" {
        " This also throws away the work every Unit has finished, and each needs its reviews and approvals again.".to_owned()
    } else {
        format!(
            " This also throws away every Unit's finished {} work, and each needs its review and approval again.",
            context.stage
        )
    }
}

/// `GUARD_REMEDY_WORDING`: a remedy's label and one line on what happens,
/// or `None` for the conductor's own work (`external-work`), which it does
/// without asking. An op not in [`REMEDY_OPS`] has no entry either.
#[must_use]
pub fn remedy_wording(op: &str, context: &RemedyWordingContext<'_>) -> Option<(String, String)> {
    let unit = context.unit.unwrap_or("this Unit");
    let (label, description) = match op {
        "present-approval-gate" => (
            "Decide at approval",
            format!(
                "I'll bring {} to you for approval with what its review raised, instead of asking for another review.",
                context.target
            ),
        ),
        "request-changes" => (
            "Request Changes",
            format!(
                "Tell me what should change in {}, and I'll revise it.{}",
                context.target,
                reset_cost(context, "reject")
            ),
        ),
        "redo-jump" => (
            "Start the stage over",
            format!(
                "I'll redo {} from the top. Your answers are kept, but you'll confirm the summary again and I'll save each of its documents again.{}",
                context.stage,
                reset_cost(context, "jump")
            ),
        ),
        "restore-or-jump" => (
            "Reopen the approved stage",
            format!(
                "{}{}",
                if context.source_unbindable {
                    format!(
                        "{} is already approved, but I can't check the project's source files. I'll reopen it and redo it from the top. To keep the approval instead, fix the list of source folders in .focus-source-paths.json so the files can be checked.",
                        context.stage
                    )
                } else {
                    format!(
                        "{} is already approved. I'll reopen it and redo it from the top. To keep the approval instead, put the files back the way they were when it was reviewed.",
                        context.stage
                    )
                },
                reset_cost(context, "jump")
            ),
        ),
        "restart-stage" => (
            "Restart the stage",
            format!(
                "I'll restart {}. Your answers are kept, and you'll be asked to confirm them again.{}",
                context.stage,
                reset_cost(context, "jump")
            ),
        ),
        "reopen-unit-step" => (
            "Do this step again for the Unit",
            format!(
                "I'll start {} again for {unit} only. The other Units keep their finished work and approvals.",
                context.stage
            ),
        ),
        "review-advisory-gate" => (
            "Decide now",
            format!(
                "I'll stop the review rounds for this piece of work and bring {} to you for approval with the reviewer's open points.",
                context.target
            ),
        ),
        "change-scope" => (
            "Switch to a scope that includes it",
            format!(
                "{} is not part of this work's scope (the set of stages it runs). I'll switch to a scope that includes it and restart it.",
                context.stage
            ),
        ),
        "restore-scope" => (
            "Switch to a scope with per-Unit stages",
            "This work's scope (the set of stages it runs) has no Construction stage that runs for each Unit, so nothing can approve this Unit. I'll switch to a scope that has one and try again.".to_owned(),
        ),
        "abort-bolt" => (
            "Restart this Unit",
            format!(
                "The automatic run for {unit} is stuck. I'll stop it, set its unfinished work aside, and start it fresh."
            ),
        ),
        "record-unit-completion" => {
            if context.code == "UNIT_COMPLETION_MISSING" {
                (
                    "Mark this Unit finished",
                    format!(
                        "The documents for {unit} are already in place. I'll record that it is finished and bring it to you for approval."
                    ),
                )
            } else {
                (
                    "Finish with the review it has",
                    format!(
                        "I'll finish {} for {unit} with the review it already has. What the review raised comes to you when its work is up for approval.",
                        context.stage
                    ),
                )
            }
        }
        "reconfirm-summary" => (
            "Confirm the summary again",
            format!(
                "I'll show you the current summary of your answers for {} to confirm, then save its documents again from it.",
                context.target
            ),
        ),
        "unset-unattended" => (
            "Stop working unattended",
            format!(
                "I'll stop working on my own and ask you what should change in {}.{}",
                context.target,
                reset_cost(context, "reject")
            ),
        ),
        "lower-fence" => (
            "Turn this check off",
            format!(
                "I'll turn {} check off for this piece of work. It is recorded, and it comes back on for the next piece of work.",
                context.fence.map_or_else(
                    || "this".to_owned(),
                    |fence| format!("the {}", check_words(fence))
                )
            ),
        ),
        _ => return None,
    };
    Some((label.to_owned(), description))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_operation_is_exactly_its_fields() {
        assert_eq!(
            parse_operation(&json!({"kind": "restart-stage", "stage": "a"})),
            Some(GuardRecoveryOperation::RestartStage { stage: "a".into() })
        );
        assert!(parse_operation(&json!({"kind": "restart-stage", "stage": "a", "x": 1})).is_none());
        assert!(parse_operation(&json!({"kind": "restart-stage", "stage": "-a"})).is_none());
        assert!(
            parse_operation(&json!({"kind": "lower-fence", "fence": "human-presence"})).is_none()
        );
        assert_eq!(
            parse_operation(&json!({"kind": "lower-fence", "fence": "review-freeze"})),
            Some(GuardRecoveryOperation::LowerFence {
                fence: Fence::ReviewFreeze
            })
        );
        assert_eq!(
            parse_operation(&json!({"kind": "review-advisory"})),
            Some(GuardRecoveryOperation::ReviewAdvisory)
        );
        assert!(parse_operation(&json!([])).is_none());
    }

    #[test]
    fn an_operation_renders_and_matches_its_own_command() {
        let lower = GuardRecoveryOperation::LowerFence {
            fence: Fence::PlanApproval,
        };
        assert_eq!(
            render(&lower, RenderMode::Native, ".claude", RenderShell::Posix).unwrap(),
            "focus engine config set guard.plan-approval off"
        );
        assert_eq!(
            render(&lower, RenderMode::Source, ".claude", RenderShell::Posix).unwrap(),
            "bun .claude/tools/focus-utility.ts config-change --guard.plan-approval off"
        );
        assert!(render(&lower, RenderMode::Source, "claude", RenderShell::Posix).is_err());
        assert!(matches_command(
            &lower,
            "focus engine config set guard.plan-approval off"
        ));
        assert!(matches_command(
            &lower,
            "bun .cursor/tools/focus-utility.ts config-change --guard.plan-approval off"
        ));
        assert!(!matches_command(
            &lower,
            "focus engine config set guard.plan-approval off "
        ));
        let abort = GuardRecoveryOperation::AbortBolt {
            unit: "u".into(),
            slug: "s".into(),
        };
        let rendered = render(
            &abort,
            RenderMode::Native,
            ".claude",
            RenderShell::PowerShell,
        )
        .unwrap();
        assert_eq!(
            rendered,
            "focus engine bolt abort --name u --slug s --reason 'stale review recovery exhausted' --discard"
        );
        assert!(matches_command(&abort, &rendered));
        // A word only PowerShell quotes differently matches in its own
        // spelling too.
        let quoted = GuardRecoveryOperation::RestartStage { stage: "a".into() };
        assert_eq!(
            quote_command_argument("it's", RenderShell::Posix),
            "'it'\"'\"'s'"
        );
        assert_eq!(
            quote_command_argument("it's", RenderShell::PowerShell),
            "'it''s'"
        );
        assert_eq!(quote_command_argument("a\tb", RenderShell::Posix), "'a?b'");
        assert_eq!(quote_command_argument("", RenderShell::Posix), "''");
        assert!(matches_command(
            &quoted,
            "focus engine orchestrate next --stage a"
        ));
        // An operation built in code is matched in either shell's quoting.
        let apostrophe = GuardRecoveryOperation::RestartStage {
            stage: "it's".into(),
        };
        assert!(matches_command(
            &apostrophe,
            "focus engine orchestrate next --stage 'it''s'"
        ));
        assert!(matches_command(
            &apostrophe,
            "focus engine orchestrate next --stage 'it'\"'\"'s'"
        ));
    }

    #[test]
    fn the_continuation_and_the_recovery_invocation_are_exact() {
        let parsed = parse_guard_restart_continuation_command(
            "focus engine jump execute --target a --direction redo --scope feature",
            None,
        )
        .unwrap();
        assert_eq!(parsed.stage, "a");
        assert_eq!(parsed.direction, RestartDirection::Redo);
        assert_eq!(parsed.scope, "feature");
        let source = "bun .claude/tools/focus-jump.ts execute --target a --direction backward --scope feature";
        assert!(parse_guard_restart_continuation_command(source, Some(".claude")).is_some());
        assert!(parse_guard_restart_continuation_command(source, None).is_none());
        assert!(
            parse_guard_restart_continuation_command(
                "focus engine jump execute --target a --direction up --scope feature",
                None
            )
            .is_none()
        );
        let words = |text: &str| text.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert!(is_guard_recovery_engine_invocation(&words(
            "engine config set guard.review-freeze off"
        )));
        assert!(!is_guard_recovery_engine_invocation(&words(
            "engine config set guard.review-freeze on"
        )));
        assert!(!is_guard_recovery_engine_invocation(&words(
            "engine config set guard.human-presence off"
        )));
        assert!(is_guard_recovery_engine_invocation(&words(
            "engine utility config-change --guard.reviewer-scope off"
        )));
        let abort = |last: &str| {
            let mut args = words("engine bolt abort --name u --slug s --reason");
            args.push("stale review recovery exhausted".to_owned());
            args.push(last.to_owned());
            args
        };
        assert!(is_guard_recovery_engine_invocation(&abort("--discard")));
        assert!(!is_guard_recovery_engine_invocation(&abort("--keep")));
        let mut bad = abort("--discard");
        bad[4] = "-u".to_owned();
        assert!(!is_guard_recovery_engine_invocation(&bad));
        // Ten words is the abort's own length: one more is not it.
        let mut long = abort("--discard");
        long.push("--x".to_owned());
        assert!(!is_guard_recovery_engine_invocation(&long));
    }

    #[test]
    fn remedies_are_matched_and_worded() {
        let restart = GuardRecoveryOperation::RestartStage { stage: "a".into() };
        assert!(matches_remedy(&restart, "redo-jump", "a", None));
        assert!(!matches_remedy(&restart, "redo-jump", "b", None));
        assert!(same(None, None));
        assert!(!same(Some(&json!({"kind": "review-advisory"})), None));
        assert!(same(
            Some(&json!({"kind": "review-advisory"})),
            Some(&json!({"kind": "review-advisory"}))
        ));
        let context = RemedyWordingContext {
            code: "X",
            stage: "Code Generation",
            target: "the plan",
            unit: None,
            fence: Some(Fence::ReviewerScope),
            source_unbindable: false,
            every_unit: true,
        };
        let (label, text) = remedy_wording("lower-fence", &context).unwrap();
        assert_eq!(label, "Turn this check off");
        assert!(text.starts_with("I'll turn the reviewer read scope check off"));
        assert!(remedy_wording("request-review", &context).is_none());
        assert!(remedy_wording("nope", &context).is_none());
        let (_, text) = remedy_wording("redo-jump", &context).unwrap();
        assert!(text.ends_with("needs its reviews and approvals again."));
        for op in REMEDY_OPS {
            let _ = remedy_wording(op, &context);
        }
    }
}
