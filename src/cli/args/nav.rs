//! The `nav` command parser — `<url> [--block <classes>]`.

use super::take_value;
use crate::error::{ObuError, Result};
use serde_json::{Value, json};

/// Parse `nav` args: <url> [--block <classes>]
pub(crate) fn parse_nav_args(args: &[String]) -> Result<Value> {
    let mut url: Option<String> = None;
    let mut block: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        if let Some(flag) = a.strip_prefix("--") {
            match flag {
                "url" => url = Some(take_value(args, &mut i, "url")?),
                "block" => block = Some(take_value(args, &mut i, "block")?),
                _ => {
                    return Err(ObuError::Usage(format!(
                        "unknown flag --{flag} for nav — see 'obu help nav'"
                    )));
                }
            }
        } else if url.is_none() {
            url = Some(a);
        } else {
            return Err(ObuError::Usage(format!(
                "unexpected extra argument '{a}' — nav takes exactly one URL"
            )));
        }
        i += 1;
    }
    let url = url.ok_or_else(|| ObuError::Usage("nav needs a URL — obu nav <url>".into()))?;
    let mut j = json!({ "action": "navigate", "url": url });
    if let Some(b) = block {
        j["block"] = json!(b);
    }
    Ok(j)
}
