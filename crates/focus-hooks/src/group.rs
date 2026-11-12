//! One registration that runs several hooks in one process
//! (`aidlc-command.ts`: `HOOK_GROUPS`; `aidlc.ts`: `runGroupMembers`).
//!
//! A host that registers several hooks for one event starts each as its own
//! process. The four checks before a tool call are one row instead, calling
//! `engine hook guard-tool-call`, and the dispatcher runs the members here.
//! Each member keeps the matcher its own row had, as a regular expression
//! over the tool name, so a tool reaches exactly the checks it reached
//! before: reads and searches go to the three that read, and only a write,
//! a shell command or a dispatch reaches the Plan Approval guard. The group
//! is a registration, not a check: it decides nothing itself.
//!
//! Every member runs, in order, even after one refuses, as the host ran
//! every hook. The call is refused when any member refuses (exit 2),
//! carrying each refusal once; a member that fails on its own refuses
//! nothing, as its own failed process refused nothing, and what a member
//! says on stderr while letting the call through is not carried at all.

use focus_js::re;
use focus_workspace::Layout;
use serde_json::{Value, json};

use crate::{Outcome, trace};

/// `PRE_TOOL_USE_GROUP_TARGET`.
pub const PRE_TOOL_USE_GROUP: &str = "guard-tool-call";

const READ_WRITE_OR_SHELL: &str =
    "^(?:Read|NotebookRead|Edit|MultiEdit|Write|NotebookEdit|LS|Glob|Grep|Bash)$";
const WRITE_SHELL_OR_DISPATCH: &str = "^(?:Edit|MultiEdit|Write|NotebookEdit|Bash|Task|Agent)$";

/// A member of a group: the hook's name and the tools it runs for.
pub type Member = (&'static str, &'static str);

/// `hookGroupMembers`: the hooks a registration runs, in order, or nothing
/// when the name is not a group.
#[must_use]
pub fn members(target: &str) -> Option<&'static [Member]> {
    const PRE_TOOL_USE: [Member; 4] = [
        ("state-transition-guard", READ_WRITE_OR_SHELL),
        ("reviewer-scope", READ_WRITE_OR_SHELL),
        ("review-freeze", READ_WRITE_OR_SHELL),
        ("plan-approval-guard", WRITE_SHELL_OR_DISPATCH),
    ];
    (target == PRE_TOOL_USE_GROUP).then_some(&PRE_TOOL_USE)
}

/// `runGroupMembers`, with `run` running one member on the payload.
#[must_use]
pub fn run(
    members: &[Member],
    input: &str,
    layout: &Layout,
    run: &dyn Fn(&str, &str, &Layout) -> Outcome,
) -> Outcome {
    // A payload this process cannot read goes to every member, which judges
    // it; so does one that names no tool.
    let tool = focus_js::json::parse(input).ok().and_then(|parsed| {
        parsed
            .get("tool_name")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let mut out = Outcome::exit(0);
    let mut refusals: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for (hook, matcher) in members {
        if tool
            .as_deref()
            .is_some_and(|tool| !re::test(matcher, "", tool))
        {
            continue;
        }
        trace::trace("hook-group-member-begin", &[("hook", json!(hook))]);
        let said = run(hook, input, layout);
        trace::trace(
            "hook-group-member-end",
            &[("hook", json!(hook)), ("code", json!(said.code))],
        );
        // What a member answers on stdout is its own answer, as written.
        out.stdout.push_str(&said.stdout);
        let reasons = match said.code {
            0 => continue,
            2 => {
                out.code = 2;
                &mut refusals
            }
            code => {
                if out.code == 0 {
                    out.code = code;
                }
                &mut failures
            }
        };
        if !said.stderr.is_empty() && !reasons.contains(&said.stderr) {
            reasons.push(said.stderr);
        }
    }
    out.stderr = if out.code == 2 { refusals } else { failures }.concat();
    out
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn layout() -> Layout {
        Layout::new("/nowhere", focus_harness::Harness::Claude)
    }

    fn said(code: i32, stdout: &str, stderr: &str) -> Outcome {
        Outcome {
            code,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
        }
    }

    /// The members that ran for a payload, each answering nothing.
    fn reached(input: &str) -> Vec<String> {
        let ran = RefCell::new(Vec::new());
        let out = run(
            members(PRE_TOOL_USE_GROUP).unwrap(),
            input,
            &layout(),
            &|hook, _, _| {
                ran.borrow_mut().push(hook.to_owned());
                Outcome::exit(0)
            },
        );
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (0, "", "")
        );
        ran.into_inner()
    }

    #[test]
    fn a_tool_reaches_the_checks_its_own_row_matched() {
        let readers = ["state-transition-guard", "reviewer-scope", "review-freeze"];
        let every = [readers.as_slice(), &["plan-approval-guard"]].concat();
        for tool in ["Read", "NotebookRead", "LS", "Glob", "Grep"] {
            assert_eq!(
                reached(&format!(r#"{{"tool_name":"{tool}"}}"#)),
                readers,
                "{tool}"
            );
        }
        for tool in ["Edit", "MultiEdit", "Write", "NotebookEdit", "Bash"] {
            assert_eq!(
                reached(&format!(r#"{{"tool_name":"{tool}"}}"#)),
                every,
                "{tool}"
            );
        }
        for tool in ["Task", "Agent"] {
            assert_eq!(
                reached(&format!(r#"{{"tool_name":"{tool}"}}"#)),
                ["plan-approval-guard"],
                "{tool}"
            );
        }
        // A tool no row matched reaches none, and the whole name is matched.
        for tool in [
            "TodoWrite",
            "",
            "write",
            "Write ",
            "xWrite",
            "Bash\n",
            "mcp__x__Write",
        ] {
            let payload = focus_js::json::stringify(&json!({ "tool_name": tool }));
            assert_eq!(reached(&payload), [""; 0], "{tool:?}");
        }
        // A payload that names no tool, or that cannot be read, goes to
        // every member, which judges it.
        for payload in [
            "{}",
            r#"{"tool_name":3}"#,
            r#"{"tool_name":null}"#,
            "null",
            "[1]",
            r#""Write""#,
            "{nope",
            "",
        ] {
            assert_eq!(reached(payload), every, "{payload}");
        }
        assert!(members("plan-approval-guard").is_none());
    }

    /// The group's answer when its members, in order, answer `answers`.
    fn answer(answers: &[Outcome]) -> (i32, String, String) {
        let next = RefCell::new(0);
        let out = run(
            members(PRE_TOOL_USE_GROUP).unwrap(),
            r#"{"tool_name":"Write"}"#,
            &layout(),
            &|_, _, _| {
                let index = next.replace_with(|index| *index + 1);
                answers.get(index).map_or_else(
                    || Outcome::exit(0),
                    |one| said(one.code, &one.stdout, &one.stderr),
                )
            },
        );
        assert_eq!(*next.borrow(), 4, "every member runs");
        (out.code, out.stdout, out.stderr)
    }

    #[test]
    fn a_refusal_is_carried_once_and_a_failure_refuses_nothing() {
        // Any refusal refuses the call; each is said once, in order, and a
        // failure beside them is not said.
        assert_eq!(
            answer(&[
                said(2, "", "one\n"),
                said(1, "", "failed\n"),
                said(2, "", "two\n"),
                said(2, "", "one\n"),
            ]),
            (2, String::new(), "one\ntwo\n".to_owned())
        );
        assert_eq!(
            answer(&[
                said(3, "", "failed\n"),
                said(0, "", ""),
                said(2, "", "no\n")
            ]),
            (2, String::new(), "no\n".to_owned())
        );
        // With no refusal the first failure's code stands, and each failure
        // is said once.
        assert_eq!(
            answer(&[
                said(0, "", ""),
                said(1, "", "a\n"),
                said(3, "", "b\n"),
                said(1, "", "a\n"),
            ]),
            (1, String::new(), "a\nb\n".to_owned())
        );
        // What a member says while letting the call through is not carried;
        // what it answers on stdout is, whatever its code.
        assert_eq!(
            answer(&[
                said(0, "aside\n", "a warning\n"),
                said(2, "own answer\n", "")
            ]),
            (2, "aside\nown answer\n".to_owned(), String::new())
        );
        assert_eq!(
            answer(&[said(0, "", "a warning\n")]),
            (0, String::new(), String::new())
        );
    }
}
