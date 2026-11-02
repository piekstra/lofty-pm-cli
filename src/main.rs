//! `lofty-pm` — agent-first Lofty governance assistant.
//! Conforms to piekstra-cli/1. Holds no credentials; mail via `gro`,
//! Lofty data via `lofty`, votes cast by the human via the email link.

mod clock;
mod commands;
mod config;
mod deadline;
mod gmail;
mod model;
mod parse;
mod store;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use pk_cli_auth::{AuthMethod, AuthStatus};
use pk_cli_config::ConfigStore;
use pk_cli_core::info::{AuthInfo, CliInfo};
use pk_cli_core::{output, CliError, CommonArgs};
use pk_cli_selfupdate::{SelfUpdateArgs, Updater};

use commands::{decide, digest, init, ledger, pending, recommend, scan, stance, verify, votes};
use config::Config;
use model::{Actor, LedgerEntry};
use store::Store;

const BIN: &str = "lofty-pm";
const REPO: &str = "piekstra/lofty-pm-cli";

#[derive(Parser, Debug)]
#[command(name = BIN, version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Credential state (this CLI needs none of its own).
    #[command(subcommand)]
    Auth(AuthCmd),
    /// Non-secret settings.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Update to the latest release from GitHub.
    SelfUpdate(SelfUpdateArgs),
    /// Print a shell completion script.
    Completions { shell: Shell },
    /// Machine-readable capability discovery (cli-info/v1).
    Info,
    /// Sweep Gmail for governance votes, updates, and results.
    Scan(scan::Args),
    /// Investment stance: interview questions, answers, and resolved views.
    #[command(subcommand)]
    Stance(stance::Cmd),
    /// Tracked votes: list/show/context, and amend a manual-parse record.
    #[command(subcommand)]
    Votes(votes::Cmd),
    /// The agent's recommendation for how to vote (reasoning via file/stdin).
    Recommend(recommend::Args),
    /// The human's decision on a recommended vote: accept, override, or
    /// decline to vote at all.
    Decide(decide::Args),
    /// The work queue: votes awaiting the agent, votes awaiting the owner,
    /// and leveled deadline reminders; `pending ack` silences a level.
    Pending(pending::Cmd),
    /// Corpus items (updates/results) grouped by property, newest first.
    Digest(digest::Args),
    /// The append-only transition/event ledger, oldest first.
    Ledger(ledger::Args),
    /// ToS/compliance verification records that double-gate auto mode.
    #[command(subcommand)]
    Verify(verify::Cmd),
    /// Readiness wizard: `gro`/`lofty` presence and auth, mailbox coverage,
    /// and data-dir writability. A report, not a gate — exits 0 even when
    /// checks fail.
    Init(init::Args),
}

#[derive(Subcommand, Debug)]
enum AuthCmd {
    /// Report credential state (auth-status/v1). Always credential-free.
    Status,
}

#[derive(Subcommand, Debug)]
enum ConfigCmd {
    /// Print the resolved config file path.
    Path,
    /// Show the effective configuration.
    Show,
    /// Set a key (see KNOWN_KEYS; `mode.<property> auto` is double-gated:
    /// all three `verify` items recorded, plus `--acknowledge-risk`).
    Set {
        key: String,
        value: String,
        /// Required when enabling auto mode.
        #[arg(long)]
        acknowledge_risk: bool,
    },
    /// Remove a config key.
    Unset { key: String },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        std::process::exit(output::fail(&e, cli.common.json));
    }
}

fn run(cli: &Cli) -> Result<(), CliError> {
    let store = ConfigStore::new(BIN);
    match &cli.command {
        Command::Auth(AuthCmd::Status) => {
            AuthStatus::new(false, true, AuthMethod::None).emit(cli.common.json);
            Ok(())
        }
        Command::Config(cmd) => config_cmd(cli, cmd, &store),
        Command::SelfUpdate(args) => Updater {
            repo: REPO.into(),
            binary: BIN.into(),
            target: env!("BUILD_TARGET").into(),
            current: env!("CARGO_PKG_VERSION").into(),
        }
        .run(args, cli.common.json, cli.common.quiet),
        Command::Completions { shell } => {
            clap_complete::generate(*shell, &mut Cli::command(), BIN, &mut std::io::stdout());
            Ok(())
        }
        Command::Info => {
            let info = CliInfo::new(
                BIN,
                env!("CARGO_PKG_VERSION"),
                &format!("https://github.com/{REPO}"),
                AuthInfo {
                    required: false,
                    method: "none".into(),
                    login_hint: None,
                },
                &[
                    "scan",
                    "votes",
                    "recommend",
                    "decide",
                    "pending",
                    "stance",
                    "digest",
                    "ledger",
                    "verify",
                    "init",
                ],
            );
            output::json(&serde_json::to_value(&info).unwrap());
            Ok(())
        }
        Command::Scan(args) => {
            let cfg: Config = store.load()?;
            let data_store = Store::open()?;
            let ctx = commands::Ctx {
                common: &cli.common,
                cfg: &cfg,
                store: &data_store,
            };
            scan::run(&ctx, args)
        }
        // `stance questions` is documented as "Works without a store" (see
        // `stance::Cmd::Questions`'s doc comment) — dispatch it before ever
        // building a `Config`/`Store`, so listing the interview questions
        // can never create `~/.local/share/lofty-pm/{votes,corpus}` (or any
        // other data-dir side effect) on a machine that's never run the
        // rest of the CLI.
        Command::Stance(stance::Cmd::Questions) => {
            stance::questions(&cli.common);
            Ok(())
        }
        Command::Stance(cmd) => {
            let cfg: Config = store.load()?;
            let data_store = Store::open()?;
            let ctx = commands::Ctx {
                common: &cli.common,
                cfg: &cfg,
                store: &data_store,
            };
            stance::run(&ctx, cmd)
        }
        Command::Votes(cmd) => {
            let cfg: Config = store.load()?;
            let data_store = Store::open()?;
            let ctx = commands::Ctx {
                common: &cli.common,
                cfg: &cfg,
                store: &data_store,
            };
            votes::run(&ctx, cmd)
        }
        Command::Recommend(args) => {
            let cfg: Config = store.load()?;
            let data_store = Store::open()?;
            let ctx = commands::Ctx {
                common: &cli.common,
                cfg: &cfg,
                store: &data_store,
            };
            recommend::run(&ctx, args)
        }
        Command::Decide(args) => {
            let cfg: Config = store.load()?;
            let data_store = Store::open()?;
            let ctx = commands::Ctx {
                common: &cli.common,
                cfg: &cfg,
                store: &data_store,
            };
            decide::run(&ctx, args)
        }
        Command::Pending(cmd) => {
            let cfg: Config = store.load()?;
            let data_store = Store::open()?;
            let ctx = commands::Ctx {
                common: &cli.common,
                cfg: &cfg,
                store: &data_store,
            };
            pending::run(&ctx, cmd)
        }
        Command::Digest(args) => {
            let cfg: Config = store.load()?;
            let data_store = Store::open()?;
            let ctx = commands::Ctx {
                common: &cli.common,
                cfg: &cfg,
                store: &data_store,
            };
            digest::run(&ctx, args)
        }
        Command::Ledger(args) => {
            let cfg: Config = store.load()?;
            let data_store = Store::open()?;
            let ctx = commands::Ctx {
                common: &cli.common,
                cfg: &cfg,
                store: &data_store,
            };
            ledger::run(&ctx, args)
        }
        Command::Verify(cmd) => {
            let cfg: Config = store.load()?;
            let data_store = Store::open()?;
            let ctx = commands::Ctx {
                common: &cli.common,
                cfg: &cfg,
                store: &data_store,
            };
            verify::run(&ctx, cmd)
        }
        Command::Init(args) => {
            // Deliberately no `Store::open()` here: `data_dir_writable` is
            // one of init's own reported checks, not a precondition for
            // running the command (see `init::run`'s doc comment).
            let cfg: Config = store.load()?;
            init::run(&cli.common, &cfg, args)
        }
    }
}

fn config_cmd(cli: &Cli, cmd: &ConfigCmd, store: &ConfigStore) -> Result<(), CliError> {
    match cmd {
        ConfigCmd::Path => {
            let path = store.path()?;
            if cli.common.json {
                output::json(&serde_json::json!({
                    "schema": "config-path/v1",
                    "path": path.display().to_string(),
                }));
            } else {
                println!("{}", path.display());
            }
            Ok(())
        }
        ConfigCmd::Show => {
            let cfg: Config = store.load()?;
            let v = serde_json::to_value(&cfg).unwrap_or_default();
            if cli.common.json {
                let mut tagged = serde_json::Map::new();
                tagged.insert(
                    "schema".into(),
                    serde_json::Value::String("config/v1".into()),
                );
                match v {
                    serde_json::Value::Object(m) => tagged.extend(m),
                    other => {
                        tagged.insert("data".into(), other);
                    }
                }
                output::json(&serde_json::Value::Object(tagged));
            } else {
                output::render(&v);
            }
            Ok(())
        }
        ConfigCmd::Set {
            key,
            value,
            acknowledge_risk,
        } => {
            let mut cfg: Config = store.load()?;
            let now_auto = set_key(&mut cfg, key, value, *acknowledge_risk)?;
            store.save(&cfg)?;
            if now_auto && !cli.common.quiet {
                let prop = key.strip_prefix("mode.").unwrap_or(key.as_str());
                eprintln!(
                    "auto mode enabled for {prop} — at your own risk; routine-tier votes only; see docs/compliance.md"
                );
            }
            Ok(())
        }
        ConfigCmd::Unset { key } => {
            let mut cfg: Config = store.load()?;
            unset_key(&mut cfg, key)?;
            store.save(&cfg)
        }
    }
}

/// Set a config key. Returns `true` when this call just turned a property's
/// mode to `auto` (the signal `config_cmd` uses to print the at-enable
/// acknowledgment); `false` for every other key/value.
///
/// The data store is opened lazily, only on the two paths that need it:
/// `mode.<property> auto`'s verification check (which then reuses the same
/// handle to log `auto-mode-enabled`), and `mode.<property> recommend` when
/// the property was previously `auto` (to log `auto-mode-disabled` — the
/// mirror image of enabling, so disabling auto mode leaves an audit trail
/// too). Every other key/value — including `mode.<property> recommend` when
/// the property wasn't already `auto` — must never touch the data dir.
fn set_key(
    cfg: &mut Config,
    key: &str,
    value: &str,
    acknowledge_risk: bool,
) -> Result<bool, CliError> {
    if let Some(prop) = key.strip_prefix("mode.") {
        if !matches!(value, "recommend" | "auto") {
            return Err(CliError::Usage("mode must be recommend|auto".into()));
        }
        let was_auto = cfg.mode_for(prop) == config::Mode::Auto;
        // `recommend` is never gated — only enabling `auto` is. Check
        // verifications before --acknowledge-risk: a caller who's verified
        // nothing shouldn't see a risk-acknowledgment error and think
        // that's the only thing standing in their way.
        if value == "auto" {
            let data_store = Store::open()?;
            let verifications = data_store.load_verifications()?;
            if !verify::all_recorded(&verifications) {
                return Err(CliError::Usage(
                    "auto mode requires all three ToS verifications; run `lofty-pm verify list`"
                        .into(),
                ));
            }
            if !acknowledge_risk {
                return Err(CliError::Usage(format!(
                    "auto mode for {prop} requires --acknowledge-risk: Lofty's terms of \
                     service don't clearly address automated/agent-driven voting, so this is \
                     gray-area territory — read docs/compliance.md, then pass \
                     --acknowledge-risk to confirm"
                )));
            }
            cfg.modes.insert(prop.to_string(), value.to_string());
            // The riskiest state change in the product gets an audit trace.
            // `data_store` is already open from the verification check
            // above — reuse the handle rather than opening a second one.
            data_store.append_ledger(&LedgerEntry::event(
                None,
                Actor::Human,
                "auto-mode-enabled",
                Some(prop.to_string()),
            ))?;
            return Ok(true);
        }
        // Only `recommend` reaches here (the `matches!` guard above already
        // rejected anything else). Log the disable, mirroring the enable
        // path above, but only when this actually turns auto off — a
        // recommend-to-recommend set stays silent and store-free.
        cfg.modes.insert(prop.to_string(), value.to_string());
        if was_auto {
            let data_store = Store::open()?;
            data_store.append_ledger(&LedgerEntry::event(
                None,
                Actor::Human,
                "auto-mode-disabled",
                Some(prop.to_string()),
            ))?;
        }
        return Ok(false);
    }
    match key {
        "gro_bin" => cfg.gro_bin = Some(value.into()),
        "grw_bin" => cfg.grw_bin = Some(value.into()),
        "lofty_bin" => cfg.lofty_bin = Some(value.into()),
        "gro_ref" => cfg.gro_ref = Some(value.into()),
        "scan_query" => cfg.scan_query = Some(value.into()),
        "label_processed" => {
            cfg.label_processed = Some(
                value
                    .parse()
                    .map_err(|_| CliError::Usage("label_processed must be true|false".into()))?,
            )
        }
        "reminder_hours" => cfg.reminder_hours = Some(value.into()),
        "high_stakes_usd" => cfg.high_stakes_usd = Some(value.into()),
        other => {
            return Err(CliError::Usage(format!(
                "unknown config key `{other}` (known: {})",
                crate::config::KNOWN_KEYS.join(", ")
            )))
        }
    }
    Ok(false)
}

fn unset_key(cfg: &mut Config, key: &str) -> Result<(), CliError> {
    if let Some(prop) = key.strip_prefix("mode.") {
        cfg.modes.remove(prop);
        return Ok(());
    }
    match key {
        "gro_bin" => cfg.gro_bin = None,
        "grw_bin" => cfg.grw_bin = None,
        "lofty_bin" => cfg.lofty_bin = None,
        "gro_ref" => cfg.gro_ref = None,
        "scan_query" => cfg.scan_query = None,
        "label_processed" => cfg.label_processed = None,
        "reminder_hours" => cfg.reminder_hours = None,
        "high_stakes_usd" => cfg.high_stakes_usd = None,
        other => {
            return Err(CliError::Usage(format!(
                "unknown config key `{other}` (known: {})",
                crate::config::KNOWN_KEYS.join(", ")
            )))
        }
    }
    Ok(())
}
