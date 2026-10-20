//! `Bun.Glob`, as far as the engine's own patterns use it: a sensor's
//! `matches` against an artifact's path.
//!
//! `*` matches within one path segment, `?` one character of one, `**` any
//! run of segments, `{a,b}` one of the alternatives, `[...]` a character
//! class; `/` is the separator. The pattern is compiled to an ECMAScript
//! regular expression and matched against the whole text.

use crate::re;

/// The regular expression a glob compiles to, anchored at both ends.
fn pattern(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut out = String::from("^");
    let mut at = 0;
    let mut in_class = false;
    let mut in_braces = 0_u32;
    while at < chars.len() {
        let c = chars[at];
        if in_class {
            if c == ']' {
                in_class = false;
            }
            out.push(c);
            at += 1;
            continue;
        }
        match c {
            '*' if chars.get(at + 1) == Some(&'*') => {
                // `**/` is zero or more whole segments; a bare `**` anything.
                if chars.get(at + 2) == Some(&'/') {
                    out.push_str("(?:.*/)?");
                    at += 3;
                } else {
                    out.push_str(".*");
                    at += 2;
                }
            }
            '*' => {
                out.push_str("[^/]*");
                at += 1;
            }
            '?' => {
                out.push_str("[^/]");
                at += 1;
            }
            '[' => {
                in_class = true;
                out.push('[');
                at += 1;
            }
            '{' => {
                in_braces += 1;
                out.push_str("(?:");
                at += 1;
            }
            '}' if in_braces > 0 => {
                in_braces -= 1;
                out.push(')');
                at += 1;
            }
            ',' if in_braces > 0 => {
                out.push('|');
                at += 1;
            }
            '\\' => {
                if let Some(next) = chars.get(at + 1) {
                    out.push_str(&re::escape(&next.to_string()));
                    at += 2;
                } else {
                    out.push_str("\\\\");
                    at += 1;
                }
            }
            other => {
                out.push_str(&re::escape(&other.to_string()));
                at += 1;
            }
        }
    }
    out.push('$');
    out
}

/// `new Bun.Glob(glob).match(text)`.
#[must_use]
pub fn matches(glob: &str, text: &str) -> bool {
    re::test(&pattern(glob), "", text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engines_patterns_match_as_bun_matches_them() {
        let stores = "**/{focus-docs,intents,codekb}/**";
        assert!(matches(
            stores,
            "/p/focus/spaces/default/intents/x/ideation/a/b.md"
        ));
        assert!(matches(
            stores,
            "/p/focus/spaces/default/codekb/app/architecture.md"
        ));
        assert!(!matches(stores, "/p/src/intents.ts"));
        assert!(!matches(stores, "/p/intents"));
        assert!(matches("**/traceability.json", "/p/a/traceability.json"));
        assert!(matches("**/traceability.json", "traceability.json"));
        assert!(!matches(
            "**/traceability.json",
            "/p/a/traceability.json.bak"
        ));
        assert!(matches("**/*.{ts,js}", "/p/src/main.ts"));
        assert!(!matches("**/*.{ts,js}", "/p/src/main.rs"));
        assert!(matches("src/*.rs", "src/main.rs"));
        assert!(!matches("src/*.rs", "src/deep/main.rs"));
        assert!(matches("src/**/*.rs", "src/deep/er/main.rs"));
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "a/c"));
        assert!(matches("[ab]x", "bx"));
        assert!(matches("a.b", "a.b"));
        assert!(!matches("a.b", "axb"));
    }
}
