//! Wire-level message types for the control protocol (spec §4).

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// A COMMAND message sent to the device (spec §4.1).
#[derive(Debug, Clone, Serialize)]
pub struct Command {
    /// Sender-chosen id, echoed back in the matching result.
    pub id: u64,
    /// Method name; must be in the device `support` list.
    pub method: String,
    /// Method-specific parameter array.
    pub params: Vec<Value>,
}

/// A NOTIFICATION message pushed by the device on state change (spec §4.3).
#[derive(Debug, Clone)]
pub struct Notification {
    /// Notification method; currently always `"props"`.
    pub method: String,
    /// Property name -> value (all values are strings per spec).
    pub params: HashMap<String, String>,
}

/// A parsed line received from the device.
#[derive(Debug)]
pub(crate) enum Incoming {
    /// Successful command result (the `result` value).
    Result { id: u64, result: Value },
    /// Command failure (the `error` object).
    Error { id: u64, code: i64, message: String },
    /// Asynchronous state-change notification.
    Notification(Notification),
}

/// Parse one `\r\n`-delimited line from the device.
///
/// Results/errors carry an `id`; notifications carry `"method": "props"` and no `id`.
pub(crate) fn parse_line(line: &str) -> Result<Incoming> {
    let v: Value = serde_json::from_str(line)?;

    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        if let Some(err) = v.get("error") {
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(-1);
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            return Ok(Incoming::Error { id, code, message });
        }
        let result = v.get("result").cloned().unwrap_or(Value::Null);
        return Ok(Incoming::Result { id, result });
    }

    if v.get("method").and_then(Value::as_str) == Some("props") {
        let params = v
            .get("params")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    // Spec §4.3 says values are strings, but real firmware sends
                    // numbers (`{"bright":83}`). Coerce any scalar to its string
                    // form so downstream `parse()` works; `as_str()` alone drops
                    // numbers to "".
                    .map(|(k, val)| {
                        let s = match val {
                            Value::String(s) => s.clone(),
                            Value::Null => String::new(),
                            other => other.to_string(),
                        };
                        (k.clone(), s)
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Ok(Incoming::Notification(Notification {
            method: "props".to_string(),
            params,
        }));
    }

    Err(Error::InvalidParam(format!("unrecognized message: {line}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn command_serializes_to_spec_shape() {
        let c = Command {
            id: 1,
            method: "set_power".to_string(),
            params: vec![json!("on"), json!("smooth"), json!(500)],
        };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(s, r#"{"id":1,"method":"set_power","params":["on","smooth",500]}"#);
    }

    #[test]
    fn parses_error_result() {
        let m = parse_line(r#"{"id":2,"error":{"code":-1,"message":"unsupported method"}}"#).unwrap();
        match m {
            Incoming::Error { id, code, message } => {
                assert_eq!(id, 2);
                assert_eq!(code, -1);
                assert_eq!(message, "unsupported method");
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn parses_ok_result() {
        let m = parse_line(r#"{"id":3,"result":["on","100"]}"#).unwrap();
        match m {
            Incoming::Result { id, result } => {
                assert_eq!(id, 3);
                assert_eq!(result[0], "on");
            }
            other => panic!("expected result, got {other:?}"),
        }
    }

    #[test]
    fn parses_notification() {
        let m = parse_line(r#"{"method":"props","params":{"power":"on","bright":"10"}}"#).unwrap();
        match m {
            Incoming::Notification(n) => {
                assert_eq!(n.params.get("power").map(String::as_str), Some("on"));
                assert_eq!(n.params.get("bright").map(String::as_str), Some("10"));
            }
            other => panic!("expected notification, got {other:?}"),
        }
    }

    #[test]
    fn parses_numeric_notification_values() {
        // Real devices send prop values as JSON numbers, not strings (spec §4.3
        // says strings, but firmware sends `{"bright":83}`). They must coerce to
        // the digit string so downstream `parse()` works — not the empty string.
        let m = parse_line(r#"{"method":"props","params":{"bright":83,"rgb":16711680}}"#).unwrap();
        match m {
            Incoming::Notification(n) => {
                assert_eq!(n.params.get("bright").map(String::as_str), Some("83"));
                assert_eq!(n.params.get("rgb").map(String::as_str), Some("16711680"));
            }
            other => panic!("expected notification, got {other:?}"),
        }
    }
}
