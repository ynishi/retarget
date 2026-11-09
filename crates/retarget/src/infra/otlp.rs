//! The OpenTelemetry protocol's JSON encoding of logs (OTLP/JSON), read into
//! flat records. It knows the encoding and nothing of any harness.
//!
//! A payload is one `ExportLogsServiceRequest`: `resourceLogs[]`, each with a
//! `resource` and `scopeLogs[]`, each with `logRecords[]`. A record's
//! attributes are laid over its resource's, so a record carries every
//! attribute that applies to it. Integers may come as JSON numbers or as
//! strings, as the encoding allows.

use std::collections::BTreeMap;

use serde_json::Value;

/// Why a payload is not OTLP/JSON logs.
#[derive(Debug, thiserror::Error)]
pub enum OtlpError {
    /// It is not JSON.
    #[error("the payload is not JSON")]
    Json(#[source] serde_json::Error),
    /// It is JSON of another shape.
    #[error("the payload is not OTLP/JSON logs: {0}")]
    Shape(&'static str),
}

/// One attribute's value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AttrValue {
    Str(String),
    Int(i64),
    Double(f64),
    Bool(bool),
    /// An array, a map or bytes: kept as present, never read.
    Other,
}

/// What reading one attribute found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Lookup<T> {
    /// The attribute is not there.
    Absent,
    /// It is there and reads as the type asked for.
    Value(T),
    /// It is there and does not.
    Unreadable,
}

impl<T> Lookup<T> {
    pub(crate) fn value(self) -> Option<T> {
        match self {
            Lookup::Value(v) => Some(v),
            Lookup::Absent | Lookup::Unreadable => None,
        }
    }
}

/// One log record with its resource's attributes under its own.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LogRecord {
    pub(crate) attrs: BTreeMap<String, AttrValue>,
    pub(crate) body: Option<String>,
    /// `timeUnixNano`, or `observedTimeUnixNano` when that is zero, absent or
    /// unreadable. Unreadable when neither gives a time and one could not be
    /// read.
    pub(crate) time_nanos: Lookup<u64>,
}

impl LogRecord {
    pub(crate) fn has(&self, key: &str) -> bool {
        self.attrs.contains_key(key)
    }

    /// A string attribute; any other type reads as not a string.
    pub(crate) fn str(&self, key: &str) -> Lookup<&str> {
        match self.attrs.get(key) {
            None => Lookup::Absent,
            Some(AttrValue::Str(s)) => Lookup::Value(s),
            Some(_) => Lookup::Unreadable,
        }
    }

    /// A non-negative integer, from an integer, an integral double or a string
    /// of digits.
    pub(crate) fn uint(&self, key: &str) -> Lookup<u64> {
        match self.attrs.get(key) {
            None => Lookup::Absent,
            Some(AttrValue::Int(i)) => u64::try_from(*i).map_or(Lookup::Unreadable, Lookup::Value),
            Some(AttrValue::Double(d)) if d.fract() == 0.0 && *d >= 0.0 && *d < u64::MAX as f64 => {
                Lookup::Value(*d as u64)
            }
            Some(AttrValue::Str(s)) => s.parse().map_or(Lookup::Unreadable, Lookup::Value),
            Some(_) => Lookup::Unreadable,
        }
    }

    /// A finite number, from an integer, a double or a numeric string.
    pub(crate) fn number(&self, key: &str) -> Lookup<f64> {
        let n = match self.attrs.get(key) {
            None => return Lookup::Absent,
            Some(AttrValue::Int(i)) => *i as f64,
            Some(AttrValue::Double(d)) => *d,
            Some(AttrValue::Str(s)) => match s.parse::<f64>() {
                Ok(n) => n,
                Err(_) => return Lookup::Unreadable,
            },
            Some(_) => return Lookup::Unreadable,
        };
        if n.is_finite() {
            Lookup::Value(n)
        } else {
            Lookup::Unreadable
        }
    }

    /// A boolean, from a boolean or the strings `"true"` and `"false"`.
    pub(crate) fn bool(&self, key: &str) -> Lookup<bool> {
        match self.attrs.get(key) {
            None => Lookup::Absent,
            Some(AttrValue::Bool(b)) => Lookup::Value(*b),
            Some(AttrValue::Str(s)) if s == "true" => Lookup::Value(true),
            Some(AttrValue::Str(s)) if s == "false" => Lookup::Value(false),
            Some(_) => Lookup::Unreadable,
        }
    }
}

/// The records of one payload, in the order they appear.
pub(crate) fn decode(payload: &str) -> Result<Vec<LogRecord>, OtlpError> {
    let root: Value = serde_json::from_str(payload).map_err(OtlpError::Json)?;
    let resource_logs = root
        .get("resourceLogs")
        .and_then(Value::as_array)
        .ok_or(OtlpError::Shape("no resourceLogs array"))?;
    let mut records = Vec::new();
    for resource_log in resource_logs {
        let resource_attrs = resource_log
            .get("resource")
            .and_then(|r| r.get("attributes"))
            .map(attributes)
            .transpose()?
            .unwrap_or_default();
        for scope_log in array(resource_log, "scopeLogs")? {
            for record in array(scope_log, "logRecords")? {
                let mut attrs = resource_attrs.clone();
                if let Some(own) = record.get("attributes") {
                    attrs.extend(attributes(own)?);
                }
                let body = record
                    .get("body")
                    .and_then(|b| b.get("stringValue"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let time_nanos = match (
                    nanos(record.get("timeUnixNano")),
                    nanos(record.get("observedTimeUnixNano")),
                ) {
                    (Lookup::Value(t), _) | (_, Lookup::Value(t)) => Lookup::Value(t),
                    (Lookup::Unreadable, _) | (_, Lookup::Unreadable) => Lookup::Unreadable,
                    (Lookup::Absent, Lookup::Absent) => Lookup::Absent,
                };
                records.push(LogRecord {
                    attrs,
                    body,
                    time_nanos,
                });
            }
        }
    }
    Ok(records)
}

/// The array under `key`, empty when it is absent.
fn array<'v>(value: &'v Value, key: &str) -> Result<&'v [Value], OtlpError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(OtlpError::Shape("a list field is not an array")),
    }
}

/// The attributes of a resource or record, none when `null`, as a list field.
fn attributes(value: &Value) -> Result<BTreeMap<String, AttrValue>, OtlpError> {
    let items = match value {
        Value::Null => return Ok(BTreeMap::new()),
        Value::Array(items) => items,
        _ => return Err(OtlpError::Shape("attributes is not an array")),
    };
    let mut attrs = BTreeMap::new();
    for item in items {
        let key = item
            .get("key")
            .and_then(Value::as_str)
            .ok_or(OtlpError::Shape("an attribute has no key"))?;
        let value = item.get("value").map_or(AttrValue::Other, any_value);
        attrs.insert(key.to_owned(), value);
    }
    Ok(attrs)
}

fn any_value(value: &Value) -> AttrValue {
    if let Some(s) = value.get("stringValue").and_then(Value::as_str) {
        return AttrValue::Str(s.to_owned());
    }
    if let Some(b) = value.get("boolValue").and_then(Value::as_bool) {
        return AttrValue::Bool(b);
    }
    if let Some(i) = value.get("intValue") {
        let int = match i {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        };
        return int.map_or(AttrValue::Other, AttrValue::Int);
    }
    if let Some(d) = value.get("doubleValue").and_then(Value::as_f64) {
        return AttrValue::Double(d);
    }
    AttrValue::Other
}

/// A time in nanoseconds; zero, as the encoding uses for "not set", is absent.
fn nanos(value: Option<&Value>) -> Lookup<u64> {
    let time = match value {
        None | Some(Value::Null) => return Lookup::Absent,
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(s)) => s.parse().ok(),
        Some(_) => None,
    };
    match time {
        Some(0) => Lookup::Absent,
        Some(t) => Lookup::Value(t),
        None => Lookup::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_carries_its_resource_attributes_under_its_own() {
        let payload = r#"{"resourceLogs":[{
            "resource":{"attributes":[
                {"key":"service.name","value":{"stringValue":"claude-code"}},
                {"key":"shared","value":{"stringValue":"resource"}}]},
            "scopeLogs":[{"logRecords":[{
                "timeUnixNano":"0","observedTimeUnixNano":"1700000000000000000",
                "body":{"stringValue":"claude_code.api_request"},
                "attributes":[
                    {"key":"shared","value":{"stringValue":"record"}},
                    {"key":"input_tokens","value":{"intValue":"12"}},
                    {"key":"cost_usd","value":{"doubleValue":0.5}},
                    {"key":"success","value":{"stringValue":"true"}}]}]}]}]}"#;
        let records = decode(payload).unwrap();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.str("service.name"), Lookup::Value("claude-code"));
        assert_eq!(r.str("shared"), Lookup::Value("record"));
        assert_eq!(r.uint("input_tokens"), Lookup::Value(12));
        assert_eq!(r.number("cost_usd"), Lookup::Value(0.5));
        assert_eq!(r.bool("success"), Lookup::Value(true));
        assert_eq!(r.uint("cost_usd"), Lookup::Unreadable);
        assert_eq!(r.uint("missing"), Lookup::Absent);
        assert_eq!(r.body.as_deref(), Some("claude_code.api_request"));
        assert_eq!(r.time_nanos, Lookup::Value(1_700_000_000_000_000_000));
    }

    #[test]
    fn other_shapes_are_refused() {
        assert!(matches!(decode("not json"), Err(OtlpError::Json(_))));
        assert!(matches!(decode("{}"), Err(OtlpError::Shape(_))));
        assert!(matches!(
            decode(r#"{"resourceLogs":[{"scopeLogs":{}}]}"#),
            Err(OtlpError::Shape(_))
        ));
        assert_eq!(decode(r#"{"resourceLogs":[]}"#).unwrap(), vec![]);
        let nulls = decode(
            r#"{"resourceLogs":[{"resource":{"attributes":null},
                "scopeLogs":[{"logRecords":[{"attributes":null}]}]}]}"#,
        )
        .unwrap();
        assert_eq!(nulls.len(), 1);
        assert!(nulls[0].attrs.is_empty());
    }
}
