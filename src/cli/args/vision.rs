//! The `vision` command parser — `[url] [--marks] [--full] [--size WxH[@scale]] [--out file.png]`.

use crate::error::{ObuError, Result};
use serde_json::{json, Value};

/// Parse `vision` args into the tool's arguments.
pub(crate) fn parse_vision_args(args: &[String]) -> Result<Value> {
    let mut parsed = json!({ "marks": false });
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        let mut value = |flag: &str| {
            rest.next().cloned().ok_or_else(|| {
                ObuError::Usage(format!("{flag} needs a value — see 'obu help vision'"))
            })
        };
        match a.as_str() {
            "--marks" => parsed["marks"] = json!(true),
            "--full" => parsed["full"] = json!(true),
            "--size" => parsed["size"] = json!(value("--size")?),
            "--out" => parsed["path"] = json!(from_here(&value("--out")?)),
            s if s.starts_with("--") => {
                return Err(ObuError::Usage(format!(
                    "unknown flag {s} for vision — see 'obu help vision'"
                )))
            }
            s if parsed.get("url").is_none() => parsed["url"] = json!(s),
            s => {
                return Err(ObuError::Usage(format!(
                    "unexpected argument '{s}' for vision — obu vision [url] [--marks] [--full] [--size WxH[@scale]] [--out file.png]"
                )))
            }
        }
    }
    Ok(parsed)
}

/// A path as this command's working directory reads it. The daemon that
/// writes the file was started somewhere else.
fn from_here(path: &str) -> String {
    match std::path::absolute(path) {
        Ok(absolute) if !path.is_empty() => absolute.display().to_string(),
        _ => path.to_string(),
    }
}

/// Resolves the `path` of every `vision` step in a `run` plan against this
/// command's working directory. It walks steps and their `then`, `else` and
/// `steps` only: a step's other values are the page's data, not the plan's.
pub(crate) fn steps_from_here(steps: &mut Value) {
    let Some(steps) = steps.as_array_mut() else {
        return;
    };
    for step in steps {
        if step.get("action").and_then(Value::as_str) == Some("vision") {
            if let Some(Value::String(path)) = step.get_mut("path") {
                *path = from_here(path);
            }
        }
        for child in ["then", "else", "steps"] {
            if let Some(nested) = step.get_mut(child) {
                steps_from_here(nested);
            }
        }
    }
}
