//! Converts form values into the API's typed value objects. A port of `inputs.ts`.

use std::sync::LazyLock;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use regex::Regex;

use crate::model::InputValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputType {
    String,
    Password,
    Boolean,
    Int,
    Float,
    DateTime,
}

impl InputType {
    /// The API's name for the type. Unknown names are treated as plain strings, as the desktop app does.
    pub fn parse(name: &str) -> Self {
        match name {
            "PASSWORD" => Self::Password,
            "BOOLEAN" => Self::Boolean,
            "INT" => Self::Int,
            "FLOAT" => Self::Float,
            "DATE_TIME" => Self::DateTime,
            _ => Self::String,
        }
    }
}

/// What the user entered: text, or a toggle for booleans.
#[derive(Clone, Debug)]
pub enum Raw<'a> {
    Text(&'a str),
    Bool(bool),
}

/// Converts a form value into the API's typed value object. The error is a
/// short reason when the value doesn't fit the type.
pub fn to_input_value(kind: InputType, raw: Raw) -> Result<InputValue, String> {
    let text = match raw {
        Raw::Text(t) => t.trim().to_string(),
        Raw::Bool(b) => b.to_string(),
    };
    let value = InputValue::default();
    match kind {
        InputType::Boolean => {
            static TRUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(true|yes|y|1)$").unwrap());
            static FALSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(false|no|n|0)$").unwrap());
            if let Raw::Bool(b) = raw {
                return Ok(InputValue { boolean_value: Some(b), ..value });
            }
            if TRUE.is_match(&text) {
                Ok(InputValue { boolean_value: Some(true), ..value })
            } else if FALSE.is_match(&text) {
                Ok(InputValue { boolean_value: Some(false), ..value })
            } else {
                Err("expected true or false".into())
            }
        }
        InputType::Int => match text.parse::<i64>() {
            Ok(n) if i32::try_from(n).is_ok() => Ok(InputValue { int_value: Some(n), ..value }),
            _ => Err("expected a whole number".into()),
        },
        InputType::Float => match text.parse::<f64>() {
            Ok(n) if n.is_finite() && !text.is_empty() => Ok(InputValue { float_value: Some(n), ..value }),
            _ => Err("expected a number".into()),
        },
        InputType::DateTime => parse_date_time(&text)
            .map(|t| InputValue { date_time_value: Some(t.to_rfc3339_opts(SecondsFormat::Secs, true)), ..value })
            .ok_or_else(|| "expected a date/time, e.g. 2026-10-17T13:00:00Z".into()),
        InputType::String | InputType::Password => Ok(InputValue {
            string_value: Some(match raw {
                Raw::Text(t) => t.to_string(),
                Raw::Bool(b) => b.to_string(),
            }),
            ..value
        }),
    }
}

/// A value with an offset is taken as written; one without (e.g. 2026-10-17T13:00) is local time;
/// a bare date is midnight UTC (the same rules as JavaScript's `Date.parse`).
fn parse_date_time(text: &str) -> Option<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(text) {
        return Some(t.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
            return Local.from_local_datetime(&naive).earliest().map(|t| t.with_timezone(&Utc));
        }
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok().map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
}

/// The reason a value is invalid, or None when it's fine (or blank).
pub fn validate_input(kind: InputType, raw: Option<&str>) -> Option<String> {
    match raw {
        None | Some("") => None,
        Some(text) => to_input_value(kind, Raw::Text(text)).err(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(kind: InputType, text: &str) -> InputValue {
        to_input_value(kind, Raw::Text(text)).unwrap()
    }
    fn err(kind: InputType, text: &str) -> String {
        to_input_value(kind, Raw::Text(text)).unwrap_err()
    }

    #[test]
    fn parses_booleans_from_text_without_treating_every_non_empty_string_as_true() {
        assert_eq!(ok(InputType::Boolean, "false").boolean_value, Some(false));
        assert_eq!(ok(InputType::Boolean, "0").boolean_value, Some(false));
        assert_eq!(ok(InputType::Boolean, "True").boolean_value, Some(true));
        assert_eq!(to_input_value(InputType::Boolean, Raw::Bool(true)).unwrap().boolean_value, Some(true));
        assert_eq!(err(InputType::Boolean, "maybe"), "expected true or false");
    }

    #[test]
    fn accepts_only_whole_numbers_in_the_int32_range_for_int() {
        assert_eq!(ok(InputType::Int, "42").int_value, Some(42));
        assert_eq!(ok(InputType::Int, " -7 ").int_value, Some(-7));
        assert!(err(InputType::Int, "abc").contains("whole number"));
        assert!(err(InputType::Int, "3.5").contains("whole number"));
        assert!(err(InputType::Int, "3000000000").contains("whole number"));
    }

    #[test]
    fn parses_float_values() {
        assert_eq!(ok(InputType::Float, "2.5").float_value, Some(2.5));
        assert_eq!(err(InputType::Float, "x"), "expected a number");
        assert_eq!(err(InputType::Float, ""), "expected a number");
        assert_eq!(err(InputType::Float, "inf"), "expected a number");
    }

    #[test]
    fn normalizes_date_time_values_to_utc() {
        assert_eq!(ok(InputType::DateTime, "2026-10-17T13:00:00Z").date_time_value.as_deref(), Some("2026-10-17T13:00:00Z"));
        assert_eq!(ok(InputType::DateTime, "2026-10-17T13:00:00-04:00").date_time_value.as_deref(), Some("2026-10-17T17:00:00Z"));
        assert_eq!(ok(InputType::DateTime, "2026-10-17").date_time_value.as_deref(), Some("2026-10-17T00:00:00Z"));
        assert!(err(InputType::DateTime, "nope").contains("date/time"));
        // No offset means local time, so the result depends on the machine; it must still be a valid UTC timestamp.
        assert!(ok(InputType::DateTime, "2026-10-17T13:00").date_time_value.unwrap().ends_with('Z'));
    }

    #[test]
    fn passes_strings_and_passwords_through_unchanged() {
        assert_eq!(ok(InputType::String, "chrome").string_value.as_deref(), Some("chrome"));
        assert_eq!(ok(InputType::Password, " s3cret ").string_value.as_deref(), Some(" s3cret "));
    }

    #[test]
    fn blank_is_valid_and_invalid_values_give_a_reason() {
        assert_eq!(validate_input(InputType::Int, Some("")), None);
        assert_eq!(validate_input(InputType::Int, None), None);
        assert_eq!(validate_input(InputType::Int, Some("abc")).as_deref(), Some("expected a whole number"));
    }

    #[test]
    fn unknown_type_names_are_plain_strings() {
        assert_eq!(InputType::parse("SOMETHING_NEW"), InputType::String);
        assert_eq!(InputType::parse("DATE_TIME"), InputType::DateTime);
    }
}
