//! Port of `DateTimeConverterTool.java` — pure-Rust datetime conversion backed by `jiff`.
//!
//! All public functions return `String` using the response envelope.
//!
//! # Format keywords
//! | Keyword            | Meaning                                                     |
//! | ------------------ | ----------------------------------------------------------- |
//! | `iso` / `iso-zoned`| ISO 8601 with `[Zone/ID]` suffix (Java `ZonedDateTime`)     |
//! | `iso-offset`       | `2026-04-22T10:00:00+02:00`                                 |
//! | `iso-local`        | `2026-04-22T10:00:00`                                       |
//! | `epoch`            | Unix seconds                                                |
//! | `epochmillis`      | Unix milliseconds                                           |
//! | `rfc1123`          | `Thu, 04 Mar 2026 12:00:00 GMT`                             |
//! | anything else      | strftime / strptime pattern                                 |

use std::str::FromStr;

use jiff::civil::{Date, DateTime};
use jiff::fmt::rfc2822;
use jiff::tz::TimeZone;
use jiff::{Span, SpanRound, Timestamp, Unit, Zoned};

use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

const TOOL_CONVERT_TIMEZONE: &str = "CONVERT_TIMEZONE";
const TOOL_FORMAT_DATETIME: &str = "FORMAT_DATETIME";
const TOOL_CURRENT_DATE_TIME: &str = "CURRENT_DATE_TIME";
const TOOL_LIST_TIMEZONES: &str = "LIST_TIMEZONES";
const TOOL_DATETIME_DIFFERENCE: &str = "DATETIME_DIFFERENCE";

// --------------------------------------------------------------------------- //
//  Public API
// --------------------------------------------------------------------------- //

/// Convert a datetime string from one IANA timezone to another, returning ISO-zoned form.
pub fn convert_timezone(datetime: &str, from_timezone: &str, to_timezone: &str) -> String {
    let from_zone = match resolve_zone(TOOL_CONVERT_TIMEZONE, from_timezone) {
        Ok(zone) => zone,
        Err(msg) => return msg,
    };
    let to_zone = match resolve_zone(TOOL_CONVERT_TIMEZONE, to_timezone) {
        Ok(zone) => zone,
        Err(msg) => return msg,
    };
    match parse_datetime(TOOL_CONVERT_TIMEZONE, datetime, &from_zone) {
        Ok(source) => {
            let target = source.with_time_zone(to_zone);
            Response::ok(TOOL_CONVERT_TIMEZONE)
                .field("DATETIME", format_iso_zoned(&target))
                .field("FROM", from_timezone.to_string())
                .field("TO", to_timezone.to_string())
                .build()
        }
        Err(msg) => msg,
    }
}

/// Reformat a datetime string using explicit input/output format keywords or strftime patterns.
pub fn format_datetime(
    datetime: &str,
    input_format: &str,
    output_format: &str,
    timezone: &str,
) -> String {
    let zone = match resolve_zone(TOOL_FORMAT_DATETIME, timezone) {
        Ok(zone) => zone,
        Err(msg) => return msg,
    };
    let parsed = match parse_with_format(TOOL_FORMAT_DATETIME, datetime, input_format, &zone) {
        Ok(zoned) => zoned,
        Err(msg) => return msg,
    };
    match format_output(TOOL_FORMAT_DATETIME, &parsed, output_format) {
        Ok(text) => Response::ok(TOOL_FORMAT_DATETIME).result(text).build(),
        Err(msg) => msg,
    }
}

/// Current datetime in the given IANA timezone, rendered using a format keyword or strftime pattern.
pub fn current_datetime(timezone: &str, format: &str) -> String {
    let zone = match resolve_zone(TOOL_CURRENT_DATE_TIME, timezone) {
        Ok(zone) => zone,
        Err(msg) => return msg,
    };
    let now = Zoned::now().with_time_zone(zone);
    match format_output(TOOL_CURRENT_DATE_TIME, &now, format) {
        Ok(text) => Response::ok(TOOL_CURRENT_DATE_TIME).result(text).build(),
        Err(msg) => msg,
    }
}

/// List IANA timezone IDs, filtered by region prefix. Empty string or `"all"`
/// returns every zone. Output is a single `VALUES` field carrying a CSV.
pub fn list_timezones(region: &str) -> String {
    let trimmed = region.trim();
    let region_label = if trimmed.is_empty() { "all" } else { trimmed };
    let mut matches: Vec<String> = if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("all") {
        jiff::tz::db().available().map(|n| n.to_string()).collect()
    } else {
        let prefix = format!("{trimmed}/");
        jiff::tz::db()
            .available()
            .filter_map(|name| {
                let text = name.to_string();
                text.starts_with(&prefix).then_some(text)
            })
            .collect()
    };

    if matches.is_empty() {
        return error_with_detail(
            TOOL_LIST_TIMEZONES,
            ErrorCode::InvalidInput,
            "no timezones found for region",
            &format!("region={trimmed}"),
        );
    }

    matches.sort();
    Response::ok(TOOL_LIST_TIMEZONES)
        .field("REGION", region_label.to_string())
        .field("COUNT", matches.len().to_string())
        .field("VALUES", matches.join(","))
        .build()
}

/// Compute the positive difference between two datetimes parsed in `timezone`.
pub fn datetime_difference(datetime1: &str, datetime2: &str, timezone: &str) -> String {
    let zone = match resolve_zone(TOOL_DATETIME_DIFFERENCE, timezone) {
        Ok(zone) => zone,
        Err(msg) => return msg,
    };
    let first = match parse_datetime(TOOL_DATETIME_DIFFERENCE, datetime1, &zone) {
        Ok(zoned) => zoned,
        Err(msg) => return msg,
    };
    let second = match parse_datetime(TOOL_DATETIME_DIFFERENCE, datetime2, &zone) {
        Ok(zoned) => zoned,
        Err(msg) => return msg,
    };
    compute_difference(&first, &second)
}

// --------------------------------------------------------------------------- //
//  Zone resolution
// --------------------------------------------------------------------------- //

fn resolve_zone(tool: &str, id: &str) -> Result<TimeZone, String> {
    TimeZone::get(id).map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "timezone is not a recognized IANA zone",
            &format!("timezone={id}"),
        )
    })
}

// --------------------------------------------------------------------------- //
//  Parsing
// --------------------------------------------------------------------------- //

fn datetime_parse_error(tool: &str, raw: &str) -> String {
    error_with_detail(
        tool,
        ErrorCode::ParseError,
        "cannot parse datetime",
        &format!("datetime={raw}"),
    )
}

/// Best-effort parse accepting ISO zoned/offset/local forms plus a few common locale patterns.
fn parse_datetime(tool: &str, datetime: &str, zone: &TimeZone) -> Result<Zoned, String> {
    if let Ok(zoned) = Zoned::from_str(datetime) {
        return Ok(zoned);
    }
    if let Ok(ts) = Timestamp::from_str(datetime) {
        return Ok(ts.to_zoned(zone.clone()));
    }
    if let Ok(civil) = DateTime::from_str(datetime) {
        return civil
            .to_zoned(zone.clone())
            .map_err(|_| datetime_parse_error(tool, datetime));
    }

    const PATTERNS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S",
        "%d/%m/%Y %H:%M:%S",
        "%m/%d/%Y %H:%M:%S",
        "%d/%m/%Y",
        "%m/%d/%Y",
    ];
    for pattern in PATTERNS {
        if let Ok(civil) = DateTime::strptime(pattern, datetime) {
            return civil
                .to_zoned(zone.clone())
                .map_err(|_| datetime_parse_error(tool, datetime));
        }
    }

    if let Ok(date) = Date::from_str(datetime) {
        return date
            .to_datetime(jiff::civil::Time::midnight())
            .to_zoned(zone.clone())
            .map_err(|_| datetime_parse_error(tool, datetime));
    }

    Err(datetime_parse_error(tool, datetime))
}

fn parse_with_format(
    tool: &str,
    datetime: &str,
    input_format: &str,
    zone: &TimeZone,
) -> Result<Zoned, String> {
    match input_format.to_ascii_lowercase().as_str() {
        "iso" | "iso-zoned" | "iso-offset" | "iso-local" => parse_datetime(tool, datetime, zone),
        "epoch" => {
            let secs: i64 = datetime
                .trim()
                .parse()
                .map_err(|_| datetime_parse_error(tool, datetime))?;
            Timestamp::from_second(secs)
                .map(|ts| ts.to_zoned(zone.clone()))
                .map_err(|_| datetime_parse_error(tool, datetime))
        }
        "epochmillis" => {
            let millis: i64 = datetime
                .trim()
                .parse()
                .map_err(|_| datetime_parse_error(tool, datetime))?;
            Timestamp::from_millisecond(millis)
                .map(|ts| ts.to_zoned(zone.clone()))
                .map_err(|_| datetime_parse_error(tool, datetime))
        }
        pattern => {
            if let Ok(zoned) = Zoned::strptime(pattern, datetime) {
                return Ok(zoned);
            }
            match DateTime::strptime(pattern, datetime) {
                Ok(civil) => civil
                    .to_zoned(zone.clone())
                    .map_err(|_| datetime_parse_error(tool, datetime)),
                Err(_) => Err(error_with_detail(
                    tool,
                    ErrorCode::InvalidInput,
                    "format pattern rejected the datetime",
                    &format!("format={pattern}"),
                )),
            }
        }
    }
}

// --------------------------------------------------------------------------- //
//  Output formatting
// --------------------------------------------------------------------------- //

fn format_output(tool: &str, zoned: &Zoned, format: &str) -> Result<String, String> {
    Ok(match format.to_ascii_lowercase().as_str() {
        "iso" | "iso-zoned" => format_iso_zoned(zoned),
        "iso-offset" => format_iso_offset(zoned),
        "iso-local" => zoned.datetime().to_string(),
        "epoch" => zoned.timestamp().as_second().to_string(),
        "epochmillis" => zoned.timestamp().as_millisecond().to_string(),
        "rfc1123" => rfc2822::to_string(zoned).map_err(|_| {
            error_with_detail(
                tool,
                ErrorCode::InvalidInput,
                "format pattern rejected the datetime",
                "format=rfc1123",
            )
        })?,
        pattern => zoned.strftime(pattern).to_string(),
    })
}

fn format_iso_zoned(zoned: &Zoned) -> String {
    zoned.to_string()
}

fn format_iso_offset(zoned: &Zoned) -> String {
    let full = zoned.to_string();
    match full.find('[') {
        Some(idx) => full[..idx].to_string(),
        None => full,
    }
}

// --------------------------------------------------------------------------- //
//  Difference
// --------------------------------------------------------------------------- //

fn compute_difference(first: &Zoned, second: &Zoned) -> String {
    let (earlier, later) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };

    let span: Span = match later
        .since((Unit::Year, earlier))
        .and_then(|s| s.round(SpanRound::new().largest(Unit::Year).relative(earlier)))
    {
        Ok(s) => s,
        Err(e) => return error(TOOL_DATETIME_DIFFERENCE, ErrorCode::InvalidInput, &e.to_string()),
    };

    let years = span.get_years();
    let months = span.get_months();
    let days = span.get_days();
    let hours = span.get_hours();
    let minutes = span.get_minutes();
    let seconds = span.get_seconds();
    let total_seconds = later.timestamp().as_second() - earlier.timestamp().as_second();

    Response::ok(TOOL_DATETIME_DIFFERENCE)
        .field("YEARS", years.to_string())
        .field("MONTHS", months.to_string())
        .field("DAYS", days.to_string())
        .field("HOURS", hours.to_string())
        .field("MINUTES", minutes.to_string())
        .field("SECONDS", seconds.to_string())
        .field("TOTAL_SECONDS", total_seconds.to_string())
        .build()
}

// --------------------------------------------------------------------------- //
//  Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_utc_to_tokyo() {
        let out = convert_timezone("2026-04-22T00:00:00Z", "UTC", "Asia/Tokyo");
        assert!(
            out.starts_with("CONVERT_TIMEZONE: OK | DATETIME: "),
            "got {out}"
        );
        assert!(out.contains("2026-04-22T09:00:00"), "got {out}");
        assert!(out.contains("[Asia/Tokyo]"), "got {out}");
        assert!(out.contains("| FROM: UTC | TO: Asia/Tokyo"), "got {out}");
    }

    #[test]
    fn convert_invalid_source_zone() {
        assert_eq!(
            convert_timezone("2026-04-22T00:00:00Z", "Not/AZone", "UTC"),
            "CONVERT_TIMEZONE: ERROR\nREASON: [INVALID_INPUT] timezone is not a recognized IANA zone\nDETAIL: timezone=Not/AZone"
        );
    }

    #[test]
    fn convert_parse_error() {
        assert_eq!(
            convert_timezone("not-a-date", "UTC", "UTC"),
            "CONVERT_TIMEZONE: ERROR\nREASON: [PARSE_ERROR] cannot parse datetime\nDETAIL: datetime=not-a-date"
        );
    }

    #[test]
    fn format_epoch_input_to_iso() {
        let out = format_datetime("0", "epoch", "iso", "UTC");
        assert!(
            out.starts_with("FORMAT_DATETIME: OK | RESULT: 1970-01-01T00:00:00"),
            "got {out}"
        );
        assert!(out.contains("[UTC]"), "got {out}");
    }

    #[test]
    fn format_iso_local_strips_zone() {
        assert_eq!(
            format_datetime("2026-04-22T10:00:00Z", "iso", "iso-local", "UTC"),
            "FORMAT_DATETIME: OK | RESULT: 2026-04-22T10:00:00"
        );
    }

    #[test]
    fn format_iso_offset_has_offset_no_zone() {
        let out = format_datetime("2026-04-22T10:00:00Z", "iso", "iso-offset", "UTC");
        assert!(out.starts_with("FORMAT_DATETIME: OK | RESULT: "), "got {out}");
        assert!(!out.contains('['), "got {out}");
        assert!(out.contains("+00:00") || out.contains('Z'), "got {out}");
    }

    #[test]
    fn format_rfc1123_output() {
        let out = format_datetime("2026-03-04T12:00:00Z", "iso", "rfc1123", "UTC");
        assert!(out.starts_with("FORMAT_DATETIME: OK | RESULT: "), "got {out}");
        assert!(out.contains("Mar 2026"), "got {out}");
        assert!(out.contains("12:00:00"), "got {out}");
    }

    #[test]
    fn current_datetime_uses_requested_zone() {
        let out = current_datetime("America/Sao_Paulo", "iso");
        assert!(
            out.starts_with("CURRENT_DATE_TIME: OK | RESULT: "),
            "got {out}"
        );
        assert!(out.contains("[America/Sao_Paulo]"), "got {out}");
    }

    #[test]
    fn current_datetime_invalid_zone() {
        assert_eq!(
            current_datetime("Not/AZone", "iso"),
            "CURRENT_DATE_TIME: ERROR\nREASON: [INVALID_INPUT] timezone is not a recognized IANA zone\nDETAIL: timezone=Not/AZone"
        );
    }

    #[test]
    fn list_timezones_europe() {
        let out = list_timezones("Europe");
        assert!(
            out.starts_with("LIST_TIMEZONES: OK | REGION: Europe | COUNT: "),
            "got {out}"
        );
        assert!(out.contains("Europe/Paris"), "missing Europe/Paris in {out}");
        assert!(out.contains("| VALUES: "), "got {out}");
    }

    #[test]
    fn list_timezones_all_keyword() {
        let out = list_timezones("all");
        assert!(
            out.starts_with("LIST_TIMEZONES: OK | REGION: all | COUNT: "),
            "got {out}"
        );
        // Sanity check: at least hundreds of zones in the CSV.
        let csv_start = out.find("| VALUES: ").expect("VALUES segment present");
        let csv = &out[csv_start + "| VALUES: ".len()..];
        assert!(csv.split(',').count() > 100, "only a few zones: {out}");
    }

    #[test]
    fn list_timezones_unknown_region_errors() {
        assert_eq!(
            list_timezones("Pluto"),
            "LIST_TIMEZONES: ERROR\nREASON: [INVALID_INPUT] no timezones found for region\nDETAIL: region=Pluto"
        );
    }

    #[test]
    fn datetime_difference_one_year_two_months() {
        let out = datetime_difference("2024-01-15T00:00:00", "2025-03-15T00:00:00", "UTC");
        assert!(
            out.starts_with("DATETIME_DIFFERENCE: OK | YEARS: 1 | MONTHS: 2 | DAYS: 0"),
            "got {out}"
        );
        assert!(out.contains("| TOTAL_SECONDS: "), "got {out}");
    }

    #[test]
    fn datetime_difference_negative_order_normalized() {
        let out = datetime_difference("2025-03-15T00:00:00", "2024-01-15T00:00:00", "UTC");
        assert!(
            out.starts_with("DATETIME_DIFFERENCE: OK | YEARS: 1 | MONTHS: 2 | DAYS: 0"),
            "got {out}"
        );
    }

    #[test]
    fn datetime_difference_invalid_zone() {
        assert_eq!(
            datetime_difference("2024-01-01T00:00:00", "2025-01-01T00:00:00", "Not/AZone"),
            "DATETIME_DIFFERENCE: ERROR\nREASON: [INVALID_INPUT] timezone is not a recognized IANA zone\nDETAIL: timezone=Not/AZone"
        );
    }
}
