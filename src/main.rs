use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
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
    /// By default, prints SQL to stdout. Pass --apply to execute against
    /// `target` (which must be a postgres URL).
    Patch {
        /// Desired state — file path or postgres URL.
        reference: String,
        /// Current state to be modified.
        target: String,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Execute the emitted SQL against `target`. Without this flag the
        /// SQL is only printed (a dry run).
        #[arg(long)]
        apply: bool,
    },
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
        Cmd::Patch { reference, target, config, apply } => {
            run_patch(&reference, &target, config.as_deref(), apply)
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

fn run_patch(reference: &str, target: &str, config: Option<&Path>, apply: bool) -> Result<()> {
    let r = load_side(reference, config)?;
    let t = load_side(target, config)?;
    // diff(left, right) reports "what's needed to go from left → right". We
    // want "make target match reference", so target is the *left* operand
    // here (we want changes that turn target into reference).
    let changes = diff::diff(&t, &r);
    let sql = emit::sql(&changes);

    if !apply {
        print!("{sql}");
        return Ok(());
    }

    if !looks_like_postgres_url(target) {
        bail!("--apply requires a postgres URL as target (got {target})");
    }
    if sql.trim().is_empty() {
        eprintln!("no changes to apply");
        return Ok(());
    }

    let mut client = Client::connect(target, tls::connector())
        .with_context(|| format!("connecting to {target}"))?;
    // Wrap the whole patch in a single transaction so a mid-stream failure
    // rolls back cleanly instead of leaving the target half-patched.
    let mut tx = client.transaction().context("opening transaction")?;
    tx.batch_execute(&sql).context("applying patch SQL")?;
    tx.commit().context("committing patch transaction")?;
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
