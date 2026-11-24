//! The `vision` command parser — `[--marks]`.

use crate::error::{ObuError, Result};
use serde_json::{Value, json};

/// Parse `vision` args: [--marks].
pub(crate) fn parse_vision_args(args: &[String]) -> Result<Value> {
    let mut marks = false;
    for a in args {
        match a.as_str() {
            "--marks" => marks = true,
            s if s.starts_with("--") => {
                return Err(ObuError::Usage(format!(
                    "unknown flag {s} for vision — see 'obu help vision'"
                )));
            }
            s => {
                return Err(ObuError::Usage(format!(
                    "unexpected argument '{s}' for vision — obu vision [--marks]"
                )));
            }
        }
    }
    Ok(json!({ "marks": marks }))
}
