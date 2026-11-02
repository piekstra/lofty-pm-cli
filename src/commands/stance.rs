//! `stance` — investment stance profile: versioned interview questions,
//! provenance-stamped answers (global or per-property), and resolved views.

use pk_cli_core::{CliError, CommonArgs};
use serde_json::{json, Value};

use crate::commands::{emit, Ctx};
use crate::model::{FieldMeta, StanceProfile};

/// One interview question. `options` is empty for free-text questions.
/// `set` writes an answered value into the matching `StanceProfile` field —
/// bundling it here (rather than a separate match keyed on `field`) is what
/// makes QUESTIONS the *actual* single source of truth: there is no second
/// place that has to be kept in sync by hand, and it's a compile error to
/// add a `Question` without one.
struct Question {
    id: &'static str,
    field: &'static str,
    prompt: &'static str,
    kind: &'static str,
    options: &'static [&'static str],
    set: fn(&mut StanceProfile, String),
}

/// The versioned interview: field set + allowed choice values. Adding a
/// question here (with its `set`) is the only step needed to make
/// `set`/`questions` know it.
const QUESTIONS: &[Question] = &[
    Question {
        id: "risk_tolerance",
        field: "risk_tolerance",
        prompt: "How much risk are you comfortable taking on for this stake?",
        kind: "choice",
        options: &["low", "moderate", "high"],
        set: |p, v| p.risk_tolerance = Some(v),
    },
    Question {
        id: "horizon",
        field: "horizon",
        prompt: "What's your investment horizon?",
        kind: "choice",
        options: &["short-term", "long-term", "flexible"],
        set: |p, v| p.horizon = Some(v),
    },
    Question {
        id: "orientation",
        field: "orientation",
        prompt: "Are you investing mainly for income, appreciation, or a balance of both?",
        kind: "choice",
        options: &["income", "appreciation", "balanced"],
        set: |p, v| p.orientation = Some(v),
    },
    Question {
        id: "add_capital",
        field: "add_capital",
        prompt: "How do you want to approach adding more capital over time?",
        kind: "choice",
        options: &["never", "bit-by-bit", "block-buys", "opportunistic"],
        set: |p, v| p.add_capital = Some(v),
    },
    Question {
        id: "notes",
        field: "notes",
        prompt: "Anything else worth noting about your stance here?",
        kind: "text",
        options: &[],
        set: |p, v| p.notes = Some(v),
    },
];

/// `lofty-pm stance <questions|set|show>`.
#[derive(clap::Subcommand, Debug)]
pub enum Cmd {
    /// List the versioned interview questions. Works without a store.
    Questions,
    /// Answer one interview question, globally or for one property.
    Set {
        /// Scope the answer to one property (default: global).
        #[arg(long)]
        property: Option<String>,
        /// Who answered: owner|agent (default: owner).
        #[arg(long, default_value = "owner")]
        by: String,
        /// One of the QUESTIONS fields (risk_tolerance, horizon,
        /// orientation, add_capital, notes).
        field: String,
        value: String,
    },
    /// Show the stance profile: global, and (with --property) the
    /// property's override plus the resolved merged view.
    Show {
        #[arg(long)]
        property: Option<String>,
    },
}

pub fn run(ctx: &Ctx, cmd: &Cmd) -> Result<(), CliError> {
    match cmd {
        // Dispatched store-free by `main.rs` before this is ever reached
        // (see that match's own `Command::Stance(stance::Cmd::Questions)`
        // arm) — handled here too so `run` stays correct/exhaustive on its
        // own if ever called directly (e.g. from a test).
        Cmd::Questions => {
            questions(ctx.common);
            Ok(())
        }
        Cmd::Set {
            property,
            by,
            field,
            value,
        } => set(ctx, property.as_deref(), by, field, value),
        Cmd::Show { property } => show(ctx, property.as_deref()),
    }
}

/// Lists the versioned interview questions. Takes `&CommonArgs` rather than
/// `&Ctx` on purpose: this is the one stance operation that reads no store
/// state, so it must stay callable (by `main.rs`) without opening the data
/// store at all — see `Cmd::Questions`'s doc comment.
pub fn questions(common: &CommonArgs) {
    let payload = json!({
        "questions": QUESTIONS.iter().map(|q| json!({
            "id": q.id,
            "field": q.field,
            "prompt": q.prompt,
            "kind": q.kind,
            "options": q.options,
        })).collect::<Vec<_>>(),
    });
    emit(common, "stance-questions", payload, |v| {
        for q in v["questions"].as_array().expect("array") {
            println!(
                "{}: {} ({})",
                q["field"].as_str().unwrap_or_default(),
                q["prompt"].as_str().unwrap_or_default(),
                q["kind"].as_str().unwrap_or_default(),
            );
        }
    });
}

fn set(
    ctx: &Ctx,
    property: Option<&str>,
    by: &str,
    field: &str,
    value: &str,
) -> Result<(), CliError> {
    if !matches!(by, "owner" | "agent") {
        return Err(CliError::Usage(format!(
            "--by must be owner|agent (got `{by}`)"
        )));
    }

    let question = QUESTIONS.iter().find(|q| q.field == field).ok_or_else(|| {
        CliError::Usage(format!(
            "unknown stance field `{field}` (known: {})",
            QUESTIONS
                .iter()
                .map(|q| q.field)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    if question.kind == "choice" && !question.options.contains(&value) {
        return Err(CliError::Usage(format!(
            "`{value}` is not a valid value for `{field}` (options: {})",
            question.options.join(", ")
        )));
    }

    let mut stance = ctx.store.load_stance()?;
    let profile = match property {
        Some(slug) => stance.overrides.entry(slug.to_string()).or_default(),
        None => &mut stance.global,
    };
    (question.set)(profile, value.to_string());

    let meta_key = match property {
        Some(slug) => format!("{slug}.{field}"),
        None => format!("global.{field}"),
    };
    stance.meta.insert(
        meta_key.clone(),
        FieldMeta {
            interviewed_at: crate::clock::now(),
            updated_by: by.to_string(),
        },
    );

    ctx.store.save_stance(&stance)?;

    emit(
        ctx.common,
        "stance-set",
        json!({
            "field": field,
            "value": value,
            "property": property,
            "by": by,
            "meta_key": meta_key,
        }),
        |v| {
            println!(
                "set {} = {} ({}, by {})",
                v["field"], v["value"], v["meta_key"], v["by"]
            );
        },
    );
    Ok(())
}

fn show(ctx: &Ctx, property: Option<&str>) -> Result<(), CliError> {
    let stance = ctx.store.load_stance()?;

    let mut payload = serde_json::Map::new();
    payload.insert(
        "global".into(),
        serde_json::to_value(&stance.global).expect("StanceProfile always serializes"),
    );
    payload.insert(
        "meta".into(),
        serde_json::to_value(&stance.meta).expect("meta map always serializes"),
    );
    if let Some(slug) = property {
        let overridden = stance.overrides.get(slug).cloned().unwrap_or_default();
        payload.insert("property".into(), json!(slug));
        payload.insert(
            "override".into(),
            serde_json::to_value(&overridden).expect("StanceProfile always serializes"),
        );
        payload.insert(
            "resolved".into(),
            serde_json::to_value(stance.resolved(slug)).expect("StanceProfile always serializes"),
        );
    }

    emit(ctx.common, "stance", Value::Object(payload), |v| {
        println!("global: {}", v["global"]);
        if let Some(p) = v.get("property") {
            println!("{p}: override={} resolved={}", v["override"], v["resolved"]);
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the single-source-of-truth claim in `QUESTIONS`'s doc comment:
    /// every question's `set` must actually write into the `StanceProfile`
    /// field named by its own `field`. If a future edit adds/renames a
    /// `Question` and gets `set` and `field` out of sync, this fails loudly
    /// instead of silently mis-writing (or, under the old match-based
    /// `apply`, panicking via `unreachable!()`).
    #[test]
    fn every_question_setter_writes_its_own_named_field() {
        for q in QUESTIONS {
            let mut profile = StanceProfile::default();
            let value = q.options.first().copied().unwrap_or("free text value");
            (q.set)(&mut profile, value.to_string());
            let serialized = serde_json::to_value(&profile).unwrap();
            assert_eq!(
                serialized[q.field].as_str(),
                Some(value),
                "QUESTIONS entry `{}`: `set` did not land in the `{}` field",
                q.id,
                q.field
            );
        }
    }
}
