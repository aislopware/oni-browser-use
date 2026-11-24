//! The `run` command parser — `<json-steps> | @file | - (stdin) | `--steps <json>`.

use super::{resolve_payload, take_value};
use crate::error::{ObuError, Result};
use serde_json::{Value, json};

/// Parse `run` args: <json-steps> | @file | - (stdin) | --steps <json>.
pub(crate) fn parse_run_args(args: &[String]) -> Result<Value> {
    let mut steps_raw: Option<String> = None;
    let mut slim = false;
    let mut format = "text".to_string();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        if let Some(flag) = a.strip_prefix("--") {
            match flag {
                "slim" => slim = true,
                "format" => {
                    format = take_value(args, &mut i, flag)?;
                    if !matches!(format.as_str(), "text" | "json") {
                        return Err(ObuError::Usage("run --format must be text or json".into()));
                    }
                }
                "steps" => steps_raw = Some(take_value(args, &mut i, flag)?),
                _ => {
                    return Err(ObuError::Usage(format!(
                        "unknown flag --{flag} for run — see 'obu help run'"
                    )));
                }
            }
        } else if steps_raw.is_none() {
            steps_raw = Some(a);
        } else {
            return Err(ObuError::Usage(
                "unexpected extra argument — pass steps once, or use @file / - for big payloads"
                    .into(),
            ));
        }
        i += 1;
    }

    let raw = steps_raw.ok_or_else(|| {
        ObuError::Usage(
            "run needs a JSON steps array, e.g. obu run '[{\"action\":\"click\",\"ref\":\"e5\"}]' \
             (or @steps.json / - for stdin)"
                .into(),
        )
    })?;
    let raw = resolve_payload(&raw)?;
    let steps: Value = serde_json::from_str(&raw)
        .map_err(|e| ObuError::Usage(format!("invalid steps JSON: {e}")))?;
    let arr = steps
        .as_array()
        .ok_or_else(|| ObuError::Usage("steps must be a JSON array".into()))?;
    if arr.is_empty() {
        return Err(ObuError::Usage("steps must not be empty".into()));
    }
    Ok(json!({ "steps": steps, "slim": slim, "format": format }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_run_format_is_explicit_and_validated() {
        let args = |values: &[&str]| values.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let value = parse_run_args(&args(&["--format", "json", "[{\"action\":\"see\"}]"])).unwrap();
        assert_eq!(value["format"], "json");
        assert_eq!(
            parse_run_args(&args(&["[{\"action\":\"see\"}]"])).unwrap()["format"],
            "text"
        );
        assert!(parse_run_args(&args(&["--format", "yaml", "[{\"action\":\"see\"}]"])).is_err());
        assert!(parse_run_args(&args(&["--format"])).is_err());
    }
}
