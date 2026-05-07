use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub include: Include,
    #[serde(default)]
    pub exclude: Exclude,
    #[serde(default)]
    pub options: Options,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Include {
    pub schemas: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Exclude {
    #[serde(default)]
    pub tables: Vec<String>,
    #[serde(default)]
    pub views: Vec<String>,
    #[serde(default)]
    pub functions: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Options {
    #[serde(default)]
    pub ignore_grants: bool,
    #[serde(default)]
    pub ignore_comments: bool,
    /// Globs matched against `schema.table` of partition children to skip in
    /// the snapshot. Useful when partitions are created at runtime by a
    /// stored procedure rather than by migrations — those children would
    /// otherwise show up as drift on every snapshot.
    #[serde(default)]
    pub ignore_partitions: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self { ignore_grants: false, ignore_comments: false, ignore_partitions: Vec::new() }
    }
}

impl Config {
    pub fn from_path(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: Self = toml::from_str(&raw)
            .with_context(|| format!("parsing config {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.include.schemas.is_empty() {
            anyhow::bail!("config: [include].schemas must list at least one schema");
        }
        Ok(())
    }

    pub fn build_exclude_set(patterns: &[String]) -> Result<GlobSet> {
        let mut b = GlobSetBuilder::new();
        for p in patterns {
            let g = Glob::new(p)
                .with_context(|| format!("invalid glob pattern: {p}"))?;
            b.add(g);
        }
        b.build().context("building globset")
    }
}
