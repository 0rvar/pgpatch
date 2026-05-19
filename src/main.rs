use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use pgpatch::{Config, Schema, catalog, diff, emit, render, tls};
use postgres::Client;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "pgpatch", version, about = "Snapshot and diff PostgreSQL schemas")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Snapshot a live schema to a deterministic JSON artefact.
    Snapshot {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        connection: String,
        /// Output path. `-` writes to stdout.
        #[arg(long, short = 'o', default_value = "-")]
        output: String,
    },
    /// Diff two artefacts, or an artefact against a live connection.
    /// Each side may be a path to a `.json` file or a postgres URL.
    Diff {
        left: String,
        right: String,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = DiffFormat::Text)]
        format: DiffFormat,
    },
    /// Emit (or apply) SQL DDL that brings `target` to match `reference`.
    /// Both sides may be a JSON artefact path or a postgres URL.
    /// Exactly one of `--dry-run` or `--dangerously-apply` is required:
    /// `--dry-run` prints SQL to stdout; `--dangerously-apply` executes it
    /// against `target` (which must be a postgres URL).
    #[command(group(ArgGroup::new("mode").required(true).multiple(false)))]
    Patch {
        /// Desired state — file path or postgres URL.
        reference: String,
        /// Current state to be modified.
        target: String,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Print the emitted SQL to stdout without connecting for writes.
        #[arg(long, group = "mode")]
        dry_run: bool,
        /// Execute the emitted SQL against `target` inside a transaction.
        #[arg(long, group = "mode")]
        dangerously_apply: bool,
    },
}

/// How `pgpatch patch` should treat the emitted SQL. Derived from the
/// mutually-exclusive `--dry-run` / `--dangerously-apply` CLI flags so that
/// downstream code branches on an enum value instead of a bare bool.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Mode {
    DryRun,
    DangerouslyApply,
}

#[derive(Copy, Clone, ValueEnum)]
enum DiffFormat {
    Text,
    Json,
    Sql,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Snapshot { config, connection, output } => snapshot(&config, &connection, &output),
        Cmd::Diff { left, right, config, format } => {
            run_diff(&left, &right, config.as_deref(), format)
        }
        Cmd::Patch { reference, target, config, dry_run, dangerously_apply } => {
            // clap's ArgGroup guarantees exactly one of the two flags is set,
            // but we re-derive the mode explicitly so the rest of the program
            // never sees the raw booleans.
            let mode = match (dry_run, dangerously_apply) {
                (true, false) => Mode::DryRun,
                (false, true) => Mode::DangerouslyApply,
                _ => unreachable!("clap ArgGroup enforces exactly one of --dry-run / --dangerously-apply"),
            };
            run_patch(&reference, &target, config.as_deref(), mode)
        }
    }
}

fn snapshot(config_path: &Path, connection: &str, output: &str) -> Result<()> {
    let cfg = Config::from_path(config_path)?;
    let schema = catalog::snapshot(connection, &cfg)?;
    let json = serde_json::to_string_pretty(&schema)?;
    if output == "-" {
        println!("{json}");
    } else {
        std::fs::write(output, json).with_context(|| format!("writing {output}"))?;
    }
    Ok(())
}

fn run_diff(left: &str, right: &str, config: Option<&Path>, format: DiffFormat) -> Result<()> {
    let l = load_side(left, config)?;
    let r = load_side(right, config)?;
    let changes = diff::diff(&l, &r);
    match format {
        DiffFormat::Text => print!("{}", render::text(&changes)),
        DiffFormat::Json => println!("{}", render::json(&changes)?),
        DiffFormat::Sql => print!("{}", emit::sql(&changes)),
    }
    Ok(())
}

fn run_patch(reference: &str, target: &str, config: Option<&Path>, mode: Mode) -> Result<()> {
    let r = load_side(reference, config)?;
    let t = load_side(target, config)?;
    // diff(left, right) reports "what's needed to go from left → right". We
    // want "make target match reference", so target is the *left* operand
    // here (we want changes that turn target into reference).
    let changes = diff::diff(&t, &r);
    let sql = emit::sql(&changes);

    if mode == Mode::DryRun {
        print!("{sql}");
        return Ok(());
    }

    // Defense-in-depth: the assertions below guard the apply branch against
    // a future refactor that accidentally wires `--dry-run` into a code path
    // that connects to or writes against the target database. Each frame of
    // the apply flow re-checks the mode so a wiring mistake panics loudly in
    // release builds instead of silently destroying data. Keep them as
    // `assert!` (not `debug_assert!`) so they survive `--release`.
    assert!(
        mode == Mode::DangerouslyApply,
        "run_patch apply branch entered with mode != DangerouslyApply ({:?}) — refusing to execute SQL",
        mode,
    );

    if !looks_like_postgres_url(target) {
        bail!("--dangerously-apply requires a postgres URL as target (got {target})");
    }
    if sql.trim().is_empty() {
        eprintln!("no changes to apply");
        return Ok(());
    }

    let conn = tls::parse(target);
    let connector = tls::connector(&conn).context("building TLS connector")?;
    assert!(
        mode == Mode::DangerouslyApply,
        "about to open a write connection with mode != DangerouslyApply ({:?})",
        mode,
    );
    let mut client = Client::connect(&conn.url, connector)
        .with_context(|| format!("connecting to {target}"))?;
    // Wrap the whole patch in a single transaction so a mid-stream failure
    // rolls back cleanly instead of leaving the target half-patched.
    let mut tx = client.transaction().context("opening transaction")?;
    assert!(
        mode == Mode::DangerouslyApply,
        "about to execute patch SQL with mode != DangerouslyApply ({:?})",
        mode,
    );
    tx.batch_execute(&sql).context("applying patch SQL")?;
    tx.commit().context("committing patch transaction")?;
    assert!(
        mode == Mode::DangerouslyApply,
        "committed patch transaction with mode != DangerouslyApply ({:?}) — apply branch is mis-wired",
        mode,
    );
    eprintln!("applied {} statement(s)", count_statements(&sql));
    Ok(())
}

fn count_statements(sql: &str) -> usize {
    sql.split(';')
        .filter(|s| {
            let t = s.trim();
            !t.is_empty() && !t.starts_with("--")
        })
        .count()
}

fn load_side(side: &str, config: Option<&Path>) -> Result<Schema> {
    if looks_like_postgres_url(side) {
        let Some(cfg_path) = config else {
            bail!("--config is required when a side is a postgres URL ({side})");
        };
        let cfg = Config::from_path(cfg_path)?;
        catalog::snapshot(side, &cfg)
    } else {
        let raw = std::fs::read_to_string(side).with_context(|| format!("reading {side}"))?;
        let schema: Schema = serde_json::from_str(&raw).with_context(|| format!("parsing {side}"))?;
        Ok(schema)
    }
}

fn looks_like_postgres_url(s: &str) -> bool {
    s.starts_with("postgres://") || s.starts_with("postgresql://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn patch_requires_a_mode_flag() {
        let result = Cli::try_parse_from(["pgpatch", "patch", "a.json", "b.json"]);
        let err = match result {
            Ok(_) => panic!("parsing should fail when neither --dry-run nor --dangerously-apply is set"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("--dry-run") && msg.contains("--dangerously-apply"),
            "expected error to mention both flags, got: {msg}",
        );
    }

    #[test]
    fn patch_rejects_both_mode_flags() {
        let result = Cli::try_parse_from([
            "pgpatch",
            "patch",
            "--dry-run",
            "--dangerously-apply",
            "a.json",
            "b.json",
        ]);
        let err = match result {
            Ok(_) => panic!("parsing should fail when both --dry-run and --dangerously-apply are set"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("cannot be used with") || msg.contains("conflict"),
            "expected mutually-exclusive conflict error, got: {msg}",
        );
    }

    #[test]
    fn patch_accepts_dry_run_alone() {
        let cli = Cli::try_parse_from(["pgpatch", "patch", "--dry-run", "a.json", "b.json"])
            .expect("--dry-run alone should parse");
        match cli.cmd {
            Cmd::Patch { dry_run, dangerously_apply, reference, target, .. } => {
                assert!(dry_run);
                assert!(!dangerously_apply);
                assert_eq!(reference, "a.json");
                assert_eq!(target, "b.json");
            }
            _ => panic!("expected Cmd::Patch"),
        }
    }

    #[test]
    fn patch_accepts_dangerously_apply_alone() {
        let cli = Cli::try_parse_from([
            "pgpatch",
            "patch",
            "--dangerously-apply",
            "a.json",
            "b.json",
        ])
        .expect("--dangerously-apply alone should parse");
        match cli.cmd {
            Cmd::Patch { dry_run, dangerously_apply, reference, target, .. } => {
                assert!(!dry_run);
                assert!(dangerously_apply);
                assert_eq!(reference, "a.json");
                assert_eq!(target, "b.json");
            }
            _ => panic!("expected Cmd::Patch"),
        }
    }
}
