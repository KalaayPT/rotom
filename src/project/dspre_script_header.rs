//! DSPRE plaintext `.script` header parsing for migration tooling.
//!
//! DSPRE writes a fixed comment block at the top of each exported script, including a
//! `Generated:` wall-clock time (C# `DateTime.ToString()` default, typically the Windows
//! thread culture). Rotom uses the **oldest** parseable `Generated:` among project scripts
//! as the export baseline (see `docs/dspre-onboarding-migration-plan.md`).

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use snafu::ResultExt;
use std::fs;
use std::path::PathBuf;

use super::error::{IoSnafu, Result};

/// Oldest DSPRE export timestamp discovered in a set of `.script` files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DspreExportBaseline {
    /// Earliest `Generated:` value among successfully parsed files.
    pub oldest: DateTime<Local>,
    /// One path whose header produced `oldest` (for diagnostics).
    pub sample_path: PathBuf,
}

/// Extracts and parses the `Generated:` field from the leading DSPRE header comment.
///
/// Only the start of `source` is scanned (through the closing `*/` of the header block when
/// present, otherwise the first 8192 bytes). Returns `None` if no `Generated:` line is found or
/// the timestamp cannot be parsed (day/month/year, else month/day/year, both 24-hour).
pub fn parse_dspre_generated_timestamp(source: &str) -> Option<DateTime<Local>> {
    let header = dspre_header_prefix(source);
    let generated = extract_generated_line(header)?;
    parse_generated_local_datetime(generated)
}

/// Reads each path and returns the oldest parseable `Generated:` timestamp, if any.
///
/// Scripts whose headers do not parse are skipped. I/O errors when reading a file are
/// propagated.
pub fn dspre_export_baseline_from_script_paths(
    paths: &[PathBuf],
) -> Result<Option<DspreExportBaseline>> {
    let mut best: Option<(DateTime<Local>, PathBuf)> = None;
    for path in paths {
        let source = fs::read_to_string(path).context(IoSnafu {
            action: "Failed to read",
            path: path.clone(),
        })?;
        let Some(dt) = parse_dspre_generated_timestamp(&source) else {
            continue;
        };
        if best.as_ref().is_none_or(|(t, _)| dt < *t) {
            best = Some((dt, path.clone()));
        }
    }
    Ok(best.map(|(oldest, sample_path)| DspreExportBaseline {
        oldest,
        sample_path,
    }))
}

fn dspre_header_prefix(source: &str) -> &str {
    if let Some(idx) = source.find("*/") {
        &source[..idx + 2]
    } else {
        &source[..source.len().min(8192)]
    }
}

fn extract_generated_line(header: &str) -> Option<&str> {
    for line in header.lines() {
        let trimmed = line.trim_start();
        let after_star = trimmed.strip_prefix('*').map_or(trimmed, str::trim_start);
        let Some(rest) = after_star.strip_prefix("Generated:") else {
            continue;
        };
        let rest = rest.trim();
        if !rest.is_empty() {
            return Some(rest);
        }
    }
    None
}

/// Parses a DSPRE `Generated:` value on the **local** clock.
///
/// DSPRE uses C# `DateTime.ToString()` which follows the Windows thread culture:
/// - en-US:  `5/16/2026 2:29:26 PM`    — slash, month/day/year, 12-hour AM/PM
/// - en-GB and most slash locales:
///   `09/05/2026 15:45:01`      — slash, day/month/year, 24-hour
/// - de-DE and other dot locales:
///   `16.05.2026 14:29:26`      — dot, day.month.year, 24-hour
///
/// We branch on the separator and AM/PM marker so there is no ambiguous fallback
/// ordering between day/month and month/day.
fn parse_generated_local_datetime(s: &str) -> Option<DateTime<Local>> {
    let upper = s.to_ascii_uppercase();
    let naive = if upper.ends_with("AM") || upper.ends_with("PM") {
        // en-US: month/day/year, 12-hour
        NaiveDateTime::parse_from_str(s, "%m/%d/%Y %I:%M:%S %p").ok()?
    } else if s.contains('.') {
        // de-DE and other dot-separator locales: day.month.year, 24-hour
        // No European locale uses month.day order with dots, so no ambiguity.
        NaiveDateTime::parse_from_str(s, "%d.%m.%Y %H:%M:%S").ok()?
    } else {
        // Slash locales (en-GB, fr-FR, es-ES, …): day/month/year, 24-hour.
        // Fall back to month/day for any edge-case 24-hour slash locales.
        NaiveDateTime::parse_from_str(s, "%d/%m/%Y %H:%M:%S")
            .or_else(|_| NaiveDateTime::parse_from_str(s, "%m/%d/%Y %H:%M:%S"))
            .ok()?
    };
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Some(dt),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};
    use std::io::Write;

    const PLATINUM_HEADER: &str = r"/*
 * DSPRE Script File
 * Rom ID: PokemonPlatinumUSA
 * Game: Plat
 * File: 0219
 * Generated: 09/05/2026 15:45:01
 */

Script 1:
End
";

    #[test]
    fn parse_dspre_generated_timestamp_platinum_fixture() {
        let dt = parse_dspre_generated_timestamp(PLATINUM_HEADER).expect("expected parse");
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 5);
        assert_eq!(dt.day(), 9);
        assert_eq!(dt.hour(), 15);
        assert_eq!(dt.minute(), 45);
        assert_eq!(dt.second(), 1);
    }

    #[test]
    fn parse_dspre_generated_timestamp_us_month_day_only_when_day_first_invalid() {
        // 06/15/… cannot be day-first (month 15); must use second pattern → June 15.
        let src = "/*\n * Generated: 06/15/2025 12:00:00\n*/\n";
        let dt = parse_dspre_generated_timestamp(src).unwrap();
        assert_eq!((dt.year(), dt.month(), dt.day()), (2025, 6, 15));
    }

    #[test]
    fn parse_dspre_generated_timestamp_12h_am_pm() {
        // DSPRE on en-US locale (and some others) writes 12-hour AM/PM timestamps.
        // Observed in German Diamond Rev5: "5/16/2026 2:29:26 PM"
        let src = "/*\n * Generated: 5/16/2026 2:29:26 PM\n*/\n";
        let dt = parse_dspre_generated_timestamp(src).unwrap();
        assert_eq!((dt.year(), dt.month(), dt.day()), (2026, 5, 16));
        assert_eq!((dt.hour(), dt.minute(), dt.second()), (14, 29, 26));
    }

    #[test]
    fn parse_dspre_generated_timestamp_12h_am() {
        // Month > 12 in day-first position forces month/day interpretation.
        let src = "/*\n * Generated: 3/15/2025 9:05:00 AM\n*/\n";
        let dt = parse_dspre_generated_timestamp(src).unwrap();
        assert_eq!((dt.year(), dt.month(), dt.day()), (2025, 3, 15));
        assert_eq!((dt.hour(), dt.minute(), dt.second()), (9, 5, 0));
    }

    #[test]
    fn parse_dspre_generated_timestamp_de_de_dot_separator() {
        // de-DE locale: dd.MM.yyyy HH:mm:ss
        let src = "/*\n * Generated: 16.05.2026 14:29:26\n*/\n";
        let dt = parse_dspre_generated_timestamp(src).unwrap();
        assert_eq!((dt.year(), dt.month(), dt.day()), (2026, 5, 16));
        assert_eq!((dt.hour(), dt.minute(), dt.second()), (14, 29, 26));
    }

    #[test]
    fn parse_dspre_generated_timestamp_missing_returns_none() {
        assert!(parse_dspre_generated_timestamp("Script 1:\nEnd\n").is_none());
    }

    #[test]
    fn dspre_export_baseline_from_script_paths_picks_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let older = dir.path().join("older.script");
        let newer = dir.path().join("newer.script");
        let mut f1 = std::fs::File::create(&older).unwrap();
        write!(
            f1,
            "/*\n * Generated: 01/01/2020 00:00:00\n*/\nScript 1:\nEnd\n"
        )
        .unwrap();
        let mut f2 = std::fs::File::create(&newer).unwrap();
        write!(
            f2,
            "/*\n * Generated: 06/15/2025 12:00:00\n*/\nScript 1:\nEnd\n"
        )
        .unwrap();

        let older_path = older.clone();
        let paths = vec![newer, older];
        let baseline = dspre_export_baseline_from_script_paths(&paths)
            .unwrap()
            .expect("expected baseline");
        assert_eq!(baseline.sample_path, older_path);
        assert_eq!(baseline.oldest.year(), 2020);
    }
}
