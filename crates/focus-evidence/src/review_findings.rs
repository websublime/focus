//! The findings a review states, read out of what the reviewer wrote.
//!
//! A review lists its findings under a `### Findings` heading, in one of two
//! forms. The engine's own is a report of two tables, `**Prior findings**`
//! (what became of each finding the reviewer was shown) and
//! `**New findings**` (`parseReviewerFindingsReport`, `aidlc-lib.ts`
//! L16 896). The one before it is a single table of six columns, each row
//! with its id and its status (`parseReviewSection`, L16 993).
//! [`read_findings_table`] reads whichever is there, or says why it cannot
//! be read (`readFindingsTable`, L17 133), and a table that cannot be read
//! is recorded as one finding that says so
//! (`unreadableFindingsTableFinding`, L17 091).
//!
//! # How the text is read
//!
//! As upstream reads it: the heading and the six-column table are found by
//! pattern, on the lines as written; only the report's two tables are looked
//! for among the lines a reader of the rendered review sees
//! ([`focus_markdown::visible_lines`]). The sentences are upstream's, the
//! rule, the row and the cell named, so the reviewer fixes that cell.

use focus_js::{json, re, text as js_text};
use focus_markdown::{Visible, visible_lines};
use serde_json::Value;

use crate::review_record::digest;

/// `REVIEW_FINDING_FIXED_STATUS_VALUES`.
pub const FIXED_STATUSES: [&str; 4] = ["New", "Unresolved", "Resolved", "Accepted risk"];
/// `REVIEW_FINDING_REJECTED_PREFIX`.
const REJECTED_PREFIX: &str = "Rejected: ";
/// `REVIEW_FINDING_STATUS_VALUES`: the statuses, as a person is told them.
pub const STATUS_VALUES: &str =
    "New, Unresolved, Resolved, Accepted risk, or \"Rejected: <reason>\"";

/// `REVIEW_FINDINGS_REPORT_RETRY_MESSAGE`.
pub const REPORT_RETRY_MESSAGE: &str = "the findings report could not be read. Write the whole review again with the required Prior findings and New findings tables";

/// `findingsReportUnreadableMessage`: the same sentence with the fault
/// named.
#[must_use]
pub fn report_unreadable_message(fault: &str) -> String {
    format!(
        "the findings report could not be read: {fault}. Write the whole review again with the required Prior findings and New findings tables"
    )
}

/// A text as `JSON.stringify` writes it.
fn quoted(text: &str) -> String {
    json::stringify(&Value::String(text.to_owned()))
}

/// `splitMarkdownRow`: the cells of a table row. A backslash keeps the
/// character after it, so `\|` is a bar inside a cell.
#[must_use]
pub fn split_markdown_row(line: &str) -> Vec<String> {
    let trimmed = js_text::trim(line);
    let inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let body = inner.strip_suffix('|').unwrap_or(inner);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for character in body.chars() {
        if escaped {
            cell.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '|' {
            cells.push(js_text::trim(&cell).to_owned());
            cell.clear();
        } else {
            cell.push(character);
        }
    }
    if escaped {
        cell.push('\\');
    }
    cells.push(js_text::trim(&cell).to_owned());
    cells
}

/// `validReviewFindingStatus`: one of the four fixed statuses, or a
/// rejection with its reason.
#[must_use]
pub fn valid_status(status: &str) -> bool {
    FIXED_STATUSES.contains(&status)
        || status
            .strip_prefix(REJECTED_PREFIX)
            .is_some_and(|reason| re::test(r"^\S[\s\S]*$", "", reason))
}

/// `reviewFindingFingerprint`: what a finding says, whatever its severity
/// and its status.
#[must_use]
pub fn fingerprint(id: &str, location: &str, finding: &str, required_action: &str) -> String {
    let said = Value::Array(
        [id, location, finding, required_action]
            .into_iter()
            .map(|text| Value::String(text.to_owned()))
            .collect(),
    );
    digest(json::stringify(&said).as_bytes())
}

/// `reviewSectionVerdict`: the canonical verdict line of a review section.
#[must_use]
pub fn section_verdict(review: &str) -> Option<String> {
    let found = re::find(r"^\*\*Verdict:\*\*\s*(READY|NOT-READY)\s*$", "m", review)?;
    re::group(review, &found, 1).map(str::to_owned)
}

/// `reviewFindingsSectionLines`: the lines under a review's `### Findings`
/// heading, up to the next heading of its level.
#[must_use]
pub fn findings_section_lines(review: &str) -> Option<Vec<String>> {
    let normalized = review.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let heading = lines
        .iter()
        .position(|line| re::test(r"^### Findings\s*$", "", line))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(heading + 1)
        .find(|(_, line)| line.starts_with("### "))
        .map_or(lines.len(), |(index, _)| index);
    Some(
        lines[heading + 1..end]
            .iter()
            .map(|line| (*line).to_owned())
            .collect(),
    )
}

/// What became of a finding the reviewer was shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Now {
    Fixed,
    StillApplies,
}

impl Now {
    /// The word upstream keeps it as.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::StillApplies => "still-applies",
        }
    }
}

/// `ReviewerPriorFindingReport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorReport {
    pub id: String,
    pub now: Now,
    pub severity: String,
    pub note: String,
}

/// `ReviewerNewFindingReport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReport {
    pub supplied_id: Option<String>,
    pub severity: String,
    pub location: String,
    pub finding: String,
    pub required_action: String,
}

/// `ReviewerFindingsReport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub prior: Vec<PriorReport>,
    pub new_findings: Vec<NewReport>,
    /// The report left out its Prior findings table.
    pub prior_missing: bool,
}

fn markdown_row<S: AsRef<str>>(cells: &[S]) -> String {
    let cells: Vec<&str> = cells.iter().map(AsRef::as_ref).collect();
    format!("| {} |", cells.join(" | "))
}

struct ReportTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl ReportTable {
    /// The cell of `row` under `header`: the last column of that name, as
    /// upstream's map of names keeps it.
    fn value<'r>(&self, row: &'r [String], header: &str) -> &'r str {
        self.headers
            .iter()
            .rposition(|name| name == header)
            .and_then(|index| row.get(index))
            .map_or("", |cell| js_text::trim(cell))
    }
}

fn opens_row(line: &str) -> bool {
    js_text::trim(line).starts_with('|')
}

/// `reportTable`: the table under a bold heading of the report.
fn report_table(
    lines: &[String],
    heading: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<ReportTable, String> {
    let wanted = format!("**{}**", js_text::to_lower_case(heading));
    let Some(heading_index) = lines
        .iter()
        .position(|line| js_text::to_lower_case(js_text::trim(line)) == wanted)
    else {
        return Err(report_unreadable_message(&format!(
            "the {heading} heading (**{heading}**) is missing"
        )));
    };
    let mut start = heading_index + 1;
    while lines
        .get(start)
        .is_some_and(|line| js_text::trim(line).is_empty())
    {
        start += 1;
    }
    let (Some(header_line), Some(separator_line)) = (
        lines.get(start).filter(|line| opens_row(line)),
        lines.get(start + 1).filter(|line| opens_row(line)),
    ) else {
        let dashes: Vec<&str> = required.iter().map(|_| "---").collect();
        return Err(report_unreadable_message(&format!(
            "no table follows the {heading} heading; write its header row {} and the separator row {}",
            markdown_row(required),
            markdown_row(&dashes)
        )));
    };
    let headers = split_markdown_row(header_line);
    if required
        .iter()
        .any(|header| !headers.iter().any(|name| name == header))
        || headers
            .iter()
            .any(|name| !required.contains(&name.as_str()) && !optional.contains(&name.as_str()))
    {
        let extra = if optional.is_empty() {
            String::new()
        } else {
            format!(" (an extra {} column is allowed)", optional.join(" or "))
        };
        return Err(report_unreadable_message(&format!(
            "the {heading} table header is {}; it must be {}{extra}",
            markdown_row(&headers),
            markdown_row(required)
        )));
    }
    let separator = split_markdown_row(separator_line);
    if separator.len() != headers.len()
        || separator
            .iter()
            .any(|cell| !re::test("^:?-{3,}:?$", "", cell))
    {
        return Err(report_unreadable_message(&format!(
            "the {heading} table separator row must have one |---| cell per column ({})",
            headers.len()
        )));
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    for line in &lines[start + 2..] {
        if !opens_row(line) {
            break;
        }
        let mut cells = split_markdown_row(line);
        // A row with nothing in any cell says nothing.
        if cells.iter().all(|cell| js_text::trim(cell).is_empty()) {
            continue;
        }
        if cells.len() > headers.len() {
            return Err(report_unreadable_message(&format!(
                "{heading} row {} has {} cells for {} columns; escape | inside a cell as \\| (also inside a code span)",
                rows.len() + 1,
                cells.len(),
                headers.len()
            )));
        }
        cells.resize(headers.len(), String::new());
        rows.push(cells);
    }
    Ok(ReportTable { headers, rows })
}

const PRIOR_HEADERS: [&str; 4] = ["ID", "Now", "Severity", "Note"];
const NEW_HEADERS: [&str; 4] = ["Severity", "Location", "Finding", "Required action"];

/// `parseReviewerFindingsReport`: the engine's findings report. `None`
/// when the review uses the six-column table instead.
///
/// # Errors
///
/// The report is there and cannot be read: the sentence says which rule,
/// row and cell.
pub fn parse_findings_report(review: &str) -> Result<Option<Report>, String> {
    let Some(section) = findings_section_lines(review) else {
        return Ok(None);
    };
    // (Upstream's reader never fails; the parser behind this one can, on a
    // document it cannot read, and then the report is not read either.)
    let Ok(visible) = visible_lines(&section.join("\n"), Visible::default()) else {
        return Err(REPORT_RETRY_MESSAGE.to_owned());
    };
    let has = |heading: &str| {
        visible
            .iter()
            .any(|line| js_text::to_lower_case(js_text::trim(line)) == heading)
    };
    let has_prior = has("**prior findings**");
    if !has("**new findings**") {
        if !has_prior {
            return Ok(None);
        }
        return Err(report_unreadable_message(
            "the New findings heading (**New findings**) is missing",
        ));
    }
    let prior_table = if has_prior {
        report_table(&visible, "Prior findings", &PRIOR_HEADERS, &[])?
    } else {
        ReportTable {
            headers: PRIOR_HEADERS
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            rows: Vec::new(),
        }
    };
    let new_table = report_table(&visible, "New findings", &NEW_HEADERS, &["ID"])?;
    let mut prior = Vec::new();
    for (index, row) in prior_table.rows.iter().enumerate() {
        let said = prior_table.value(row, "Now");
        let now = match js_text::to_lower_case(said).as_str() {
            "fixed" | "resolved" => Now::Fixed,
            "still applies" | "open" | "unresolved" => Now::StillApplies,
            _ => {
                return Err(report_unreadable_message(&format!(
                    "Prior findings row {} has Now \"{said}\"; write Fixed or Still applies",
                    index + 1
                )));
            }
        };
        let id = prior_table.value(row, "ID");
        if !re::test("^R-[0-9]+$", "", id) {
            return Err(report_unreadable_message(&format!(
                "Prior findings row {} has ID \"{id}\"; use the engine's R-<n> id from the review context",
                index + 1
            )));
        }
        prior.push(PriorReport {
            id: id.to_owned(),
            now,
            severity: prior_table.value(row, "Severity").to_owned(),
            note: prior_table.value(row, "Note").to_owned(),
        });
    }
    let mut new_findings = Vec::new();
    for (index, row) in new_table.rows.iter().enumerate() {
        // A placeholder row is refused: an empty table is how a review says
        // there is nothing new.
        if NEW_HEADERS
            .iter()
            .any(|header| re::test("^(?:-*|n/?a|none)$", "i", new_table.value(row, header)))
            || js_text::to_lower_case(new_table.value(row, "Finding")) == "no findings"
        {
            return Err(report_unreadable_message(&format!(
                "New findings row {} is a placeholder; leave the table empty when there is nothing new",
                index + 1
            )));
        }
        let supplied = new_table.value(row, "ID");
        new_findings.push(NewReport {
            supplied_id: (!supplied.is_empty()).then(|| supplied.to_owned()),
            severity: new_table.value(row, "Severity").to_owned(),
            location: new_table.value(row, "Location").to_owned(),
            finding: new_table.value(row, "Finding").to_owned(),
            required_action: new_table.value(row, "Required action").to_owned(),
        });
    }
    Ok(Some(Report {
        prior,
        new_findings,
        prior_missing: !has_prior,
    }))
}

/// `ReviewFinding`, as a review states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub artifact: String,
    pub unit: Option<String>,
    pub id: String,
    pub severity: String,
    pub location: String,
    pub finding: String,
    pub required_action: String,
    pub status: String,
    pub fingerprint: String,
}

impl Finding {
    #[allow(clippy::too_many_arguments)]
    fn stated(
        artifact: &str,
        unit: Option<&str>,
        id: &str,
        severity: &str,
        location: &str,
        finding: &str,
        required_action: &str,
        status: &str,
    ) -> Self {
        Self {
            artifact: artifact.to_owned(),
            unit: unit.filter(|unit| !unit.is_empty()).map(str::to_owned),
            id: id.to_owned(),
            severity: severity.to_owned(),
            location: location.to_owned(),
            finding: finding.to_owned(),
            required_action: required_action.to_owned(),
            status: status.to_owned(),
            fingerprint: fingerprint(id, location, finding, required_action),
        }
    }
}

/// What [`parse_review_section`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub verdict: Option<String>,
    pub findings: Vec<Finding>,
    /// A findings table was there at all: prose under the heading is
    /// another case than the table with no rows.
    pub table_present: bool,
}

const SECTION_HEADERS: [&str; 6] = [
    "ID",
    "Severity",
    "Location",
    "Finding",
    "Required action",
    "Status",
];

/// `parseReviewSection`: the verdict line and the six-column findings
/// table of one review section.
///
/// # Errors
///
/// A header without a column the record needs, or a malformed row: a bad
/// review is refused, not half read.
pub fn parse_review_section(
    review: &str,
    artifact: &str,
    unit: Option<&str>,
) -> Result<Section, String> {
    let verdict = section_verdict(review);
    let absent = |verdict| Section {
        verdict,
        findings: Vec::new(),
        table_present: false,
    };
    let Some(section) = findings_section_lines(review) else {
        return Ok(absent(verdict));
    };
    let table: Vec<&String> = section.iter().filter(|line| opens_row(line)).collect();
    let [header_line, _, rows @ ..] = table.as_slice() else {
        return Ok(absent(verdict));
    };
    let headers = split_markdown_row(header_line);
    let missing: Vec<&str> = SECTION_HEADERS
        .into_iter()
        .filter(|name| !headers.iter().any(|header| header == name))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{artifact}: findings table header declares {}. Expected columns: {}. Missing: {}",
            headers.join(" | "),
            SECTION_HEADERS.join(" | "),
            missing.join(", ")
        ));
    }
    // (A name written twice is its last column, as upstream's map keeps it.)
    let column = |name: &str| headers.iter().rposition(|header| header == name);
    let mut findings = Vec::new();
    for line in rows {
        let cells = split_markdown_row(line);
        if cells.len() != headers.len() {
            let row_id = column("ID")
                .and_then(|index| cells.get(index))
                .map(|cell| js_text::trim(cell))
                .filter(|cell| !cell.is_empty())
                .unwrap_or("?");
            if cells.len() < headers.len() {
                let last = cells.last().map_or("", String::as_str);
                let hint = if headers.last().is_some_and(|name| name == "Status")
                    && valid_status(last)
                {
                    format!(
                        "The last cell {} looks like Status; check earlier cells for a missing value or \"|\" separator",
                        quoted(last)
                    )
                } else {
                    "Check for a missing cell or \"|\" separator".to_owned()
                };
                return Err(format!(
                    "{artifact}#{row_id}: row has {} cells, header declares {}. Expected columns: {}. {hint}",
                    cells.len(),
                    headers.len(),
                    headers.join(" | ")
                ));
            }
            return Err(format!(
                "{artifact}#{row_id}: row has {} cells, header declares {}: {} unexpected extra cell(s)",
                cells.len(),
                headers.len(),
                cells.len() - headers.len()
            ));
        }
        let value = |name: &str| {
            column(name)
                .and_then(|index| cells.get(index))
                .map_or("", |cell| js_text::trim(cell))
        };
        let id = value("ID");
        if !re::test("^R-[0-9]+$", "", id) {
            return Err(format!("{artifact}: invalid finding ID {}", quoted(id)));
        }
        let status = value("Status");
        if !valid_status(status) {
            return Err(format!(
                "{artifact}#{id}: invalid finding status {}. Valid statuses: {STATUS_VALUES}.",
                quoted(status)
            ));
        }
        findings.push(Finding::stated(
            artifact,
            unit,
            id,
            value("Severity"),
            value("Location"),
            value("Finding"),
            value("Required action"),
            status,
        ));
    }
    Ok(Section {
        verdict,
        findings,
        table_present: true,
    })
}

const UNREADABLE_ID: &str = "R-00";
const UNREADABLE_LOCATION: &str = "review findings table";

/// `unreadableFindingsTableFinding`: the one finding that stands in for a
/// findings table that could not be read. It names why; the reviewer's rows
/// are not guessed at.
#[must_use]
pub fn unreadable_findings_table_finding(
    artifact: &str,
    reason: &str,
    unit: Option<&str>,
) -> Finding {
    Finding::stated(
        artifact,
        unit,
        UNREADABLE_ID,
        "Major",
        &format!("{artifact} > {UNREADABLE_LOCATION}"),
        // Commas keep this one cell if the finding is carried forward
        // without the table's escaping.
        &format!(
            "The reviewer's findings table could not be read, so its rows are not listed here: {}",
            re::replace(r"\s*\|\s*", "g", reason, ", ")
        ),
        "Address the reviewer's findings as written below this table.",
        "Unresolved",
    )
}

/// `isUnreadableFindingsTableFinding`.
#[must_use]
pub fn is_unreadable_findings_table_finding(id: &str, location: &str) -> bool {
    id == UNREADABLE_ID && location.ends_with(&format!(" > {UNREADABLE_LOCATION}"))
}

/// What [`read_findings_table`] gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub findings: Vec<Finding>,
    /// Why the table cannot be read.
    pub unreadable: Option<String>,
    /// The report, when the review used one and it was read.
    pub report: Option<Report>,
}

/// `readFindingsTable`: a review's findings as a record admits them: its
/// rows, or why they cannot be read. Prose under the heading, or no heading
/// at all, is not unreadable here.
#[must_use]
pub fn read_findings_table(
    review: &str,
    artifact: &str,
    verdict: Option<&str>,
    unit: Option<&str>,
) -> Table {
    let unreadable = |reason: String| Table {
        findings: Vec::new(),
        unreadable: Some(reason),
        report: None,
    };
    let rejected = verdict == Some("NOT-READY");
    match parse_findings_report(review) {
        Err(reason) => unreadable(reason),
        Ok(Some(report)) => {
            if rejected && report.prior.is_empty() && report.new_findings.is_empty() {
                return unreadable(
                    "a NOT-READY review with a findings report must record at least one finding in it"
                        .to_owned(),
                );
            }
            let findings = report
                .new_findings
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    Finding::stated(
                        artifact,
                        unit,
                        &format!("R-{:02}", index + 1),
                        &row.severity,
                        &row.location,
                        &row.finding,
                        &row.required_action,
                        "New",
                    )
                })
                .collect();
            Table {
                findings,
                unreadable: None,
                report: Some(report),
            }
        }
        Ok(None) => match parse_review_section(review, artifact, unit) {
            Err(reason) => unreadable(reason),
            Ok(parsed) if rejected && parsed.table_present && parsed.findings.is_empty() => {
                unreadable(
                    "a NOT-READY review with a findings table must record at least one finding in it"
                        .to_owned(),
                )
            }
            Ok(parsed) => Table {
                findings: parsed.findings,
                unreadable: None,
                report: None,
            },
        },
    }
}
