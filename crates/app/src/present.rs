//! Turn stored analysis records into wording and categories for display.
//! Nothing here decides lineage; it only explains what the core recorded.

/// Visual category of a lineage event or state, mapped to a color by the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Origin,
    Identity,
    Location,
    Change,
    Quiet,
    Uncertain,
    Gone,
}

/// A readable label for an event or state kind recorded by the core.
pub fn event_label(kind: &str) -> (&'static str, Tone) {
    match kind {
        "Created" => ("Created", Tone::Origin),
        "FirstObserved" => ("First seen", Tone::Uncertain),
        "MergeImported" => ("Arrived by merge", Tone::Uncertain),
        "Renamed" => ("Renamed", Tone::Identity),
        "Moved" => ("Moved file", Tone::Location),
        "SignatureChanged" => ("Signature changed", Tone::Change),
        "BodyChanged" => ("Body changed", Tone::Change),
        "TextOnlyChanged" => ("Comments or formatting", Tone::Quiet),
        "Ambiguous" => ("Ambiguous", Tone::Uncertain),
        "Unknown" => ("Not analyzed", Tone::Uncertain),
        "Incomplete" => ("Incomplete commit", Tone::Uncertain),
        "SkippedFile" => ("Skipped file", Tone::Uncertain),
        "Deleted" => ("Deleted", Tone::Gone),
        "Unchanged" => ("Unchanged", Tone::Quiet),
        _ => ("Recorded", Tone::Quiet),
    }
}

/// Short explanation shown beside an event in the timeline.
pub fn event_hint(kind: &str) -> &'static str {
    match kind {
        "Created" => "First appears at the start of the complete history.",
        "FirstObserved" => "History is incomplete here, so creation cannot be confirmed.",
        "MergeImported" => "Came in through a merge; other merge parents are not analyzed.",
        "Renamed" => "Same declaration under a new name.",
        "Moved" => "Same declaration in a different file.",
        "SignatureChanged" => "Parameters, return type, or modifiers changed.",
        "BodyChanged" => "The implementation changed.",
        "TextOnlyChanged" => "Only comments or whitespace changed.",
        "Ambiguous" => "Several earlier declarations match; none was linked automatically.",
        "Unknown" | "Incomplete" => "This commit was not fully analyzed.",
        "Unchanged" => "The declaration is identical here; other code in the file changed.",
        _ => "",
    }
}

/// One comparison behind an automatic link, as a yes/no line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub label: &'static str,
    pub held: bool,
}

/// Matching evidence for an automatic link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkEvidence {
    pub checks: Vec<Check>,
    pub score: Option<u64>,
}

/// Read the JSON evidence the index stores for automatic links. Other evidence
/// is free text and is shown as written.
pub fn link_evidence(evidence: &str) -> Option<LinkEvidence> {
    let value: serde_json::Value = serde_json::from_str(evidence).ok()?;
    let object = value.as_object()?;
    const CHECKS: [(&str, &str); 7] = [
        ("same_name", "Same name"),
        ("same_path", "Same file"),
        ("same_context", "Same module and impl context"),
        ("same_signature", "Same signature"),
        ("same_body", "Same body"),
        ("same_tokens", "Same code ignoring comments and whitespace"),
        ("same_structure", "Same structure ignoring identifiers"),
    ];
    let checks = CHECKS
        .iter()
        .filter_map(|(key, label)| {
            object
                .get(*key)?
                .as_bool()
                .map(|held| Check { label, held })
        })
        .collect::<Vec<_>>();
    if checks.is_empty() {
        return None;
    }
    Some(LinkEvidence {
        checks,
        score: object.get("score").and_then(serde_json::Value::as_u64),
    })
}

/// Readable kind of a stored candidate, from its evidence prefix.
pub fn candidate_label(evidence: &str) -> &'static str {
    match evidence.split(';').next().map(str::trim) {
        Some("PossibleCopy") => "Possible copy",
        Some("Ambiguous") => "Competing match",
        _ => "Candidate",
    }
}

pub fn short_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

pub fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or("").trim()
}

/// Relative time for recent commits and a calendar date for older ones.
pub fn when(seconds: Option<i64>, now: i64) -> String {
    let Some(seconds) = seconds else {
        return "unknown date".into();
    };
    let age = now.saturating_sub(seconds);
    match age {
        i64::MIN..=-1 => date(seconds),
        0..=59 => "just now".into(),
        60..=3_599 => plural(age / 60, "minute"),
        3_600..=86_399 => plural(age / 3_600, "hour"),
        86_400..=2_591_999 => plural(age / 86_400, "day"),
        _ => date(seconds),
    }
}

/// A count with thousands separators, such as `100,000`.
pub fn count(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Rounded remaining time for a progress estimate.
pub fn remaining(seconds: u64) -> String {
    match seconds {
        0..=9 => "a few seconds left".into(),
        10..=59 => format!("about {} seconds left", seconds.div_ceil(5) * 5),
        60..=119 => "about a minute left".into(),
        _ => format!("about {} minutes left", seconds.div_ceil(60)),
    }
}

fn plural(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}

/// UTC calendar date from Unix seconds (days-to-civil conversion).
pub fn date(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_relative_times() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_704_067_200), "2024-01-01");
        assert_eq!(date(-86_400), "1969-12-31");
        let now = 1_704_067_200;
        assert_eq!(when(Some(now - 30), now), "just now");
        assert_eq!(when(Some(now - 60), now), "1 minute ago");
        assert_eq!(when(Some(now - 7_200), now), "2 hours ago");
        assert_eq!(when(Some(now - 3 * 86_400), now), "3 days ago");
        assert_eq!(when(Some(now - 40 * 86_400), now), "2023-11-22");
        assert_eq!(when(None, now), "unknown date");
        assert_eq!(count(7), "7");
        assert_eq!(count(1_000), "1,000");
        assert_eq!(count(100_000), "100,000");
        assert_eq!(count(1_234_567), "1,234,567");
        assert_eq!(remaining(3), "a few seconds left");
        assert_eq!(remaining(41), "about 45 seconds left");
        assert_eq!(remaining(90), "about a minute left");
        assert_eq!(remaining(150), "about 3 minutes left");
    }

    #[test]
    fn stored_link_evidence_becomes_checks() {
        let evidence = r#"{"same_path":false,"same_name":true,"same_context":true,"same_signature":true,"same_body":true,"same_tokens":true,"same_structure":true,"score":90,"compared_parent":"abc","rule":2}"#;
        let parsed = link_evidence(evidence).expect("evidence");
        assert_eq!(parsed.score, Some(90));
        assert_eq!(
            parsed.checks.first(),
            Some(&Check {
                label: "Same name",
                held: true
            })
        );
        assert!(
            parsed
                .checks
                .iter()
                .any(|check| check.label == "Same file" && !check.held)
        );
        assert!(link_evidence("Compared with the next first-parent snapshot.").is_none());
        assert_eq!(
            candidate_label("PossibleCopy; Evidence { .. }; rule 2"),
            "Possible copy"
        );
        assert_eq!(
            candidate_label("Ambiguous; Evidence { .. }; rule 2"),
            "Competing match"
        );
    }

    #[test]
    fn every_recorded_kind_has_a_label() {
        for kind in [
            "Created",
            "FirstObserved",
            "MergeImported",
            "Renamed",
            "Moved",
            "SignatureChanged",
            "BodyChanged",
            "TextOnlyChanged",
            "Ambiguous",
            "Unknown",
            "Incomplete",
            "SkippedFile",
            "Deleted",
        ] {
            assert_ne!(event_label(kind).0, "Recorded", "{kind}");
        }
    }
}
