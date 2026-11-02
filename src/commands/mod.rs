//! Command modules. `emit` follows the family idiom: schema-tagged JSON in
//! --json mode, human rendering otherwise.

pub mod decide;
pub mod digest;
pub mod init;
pub mod ledger;
pub mod pending;
pub mod recommend;
pub mod scan;
pub mod stance;
pub mod verify;
pub mod votes;

use std::io::Read;

use pk_cli_core::{output, CliError, CommonArgs};
use serde_json::Value;

use crate::config::Config;
use crate::store::Store;

pub struct Ctx<'a> {
    pub common: &'a CommonArgs,
    pub cfg: &'a Config,
    pub store: &'a Store,
}

pub fn emit(common: &CommonArgs, schema: &str, payload: Value, text: impl FnOnce(&Value)) {
    if common.json {
        let mut tagged = serde_json::Map::new();
        tagged.insert("schema".into(), Value::String(format!("{schema}/v1")));
        match payload {
            Value::Object(m) => tagged.extend(m),
            other => {
                tagged.insert("data".into(), other);
            }
        }
        output::json(&Value::Object(tagged));
    } else {
        text(&payload);
    }
}

/// Read a long-form text flag's content from a file path, or from stdin
/// when `path == "-"`. Shared by `votes amend --proposal-file` and
/// `recommend --reasoning-file`: text that must never be passed inline on
/// argv (shell quoting, argv length limits, shell-history leakage). A
/// single trailing newline (and CR) is trimmed, matching
/// `pk_cli_secrets::read_stdin`'s convention.
pub fn read_file_or_stdin(path: &str) -> Result<String, CliError> {
    let buf = if path == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| CliError::Other(format!("reading stdin: {e}")))?;
        s
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::Usage(format!("reading {path}: {e}")))?
    };
    let trimmed = buf.strip_suffix('\n').unwrap_or(&buf);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    Ok(trimmed.to_string())
}
