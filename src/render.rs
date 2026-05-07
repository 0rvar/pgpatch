use crate::diff::Change;
use crate::model::Column;
use anyhow::Result;
use std::fmt::Write;

pub fn text(changes: &[Change]) -> String {
    if changes.is_empty() {
        return "no changes\n".to_string();
    }
    let mut out = String::new();
    for c in changes {
        match c {
            Change::SchemaAdded { name } => {
                let _ = writeln!(out, "+ schema {name}");
            }
            Change::SchemaRemoved { name } => {
                let _ = writeln!(out, "- schema {name}");
            }
            Change::TableAdded { qual, .. } => {
                let _ = writeln!(out, "+ table {qual}");
            }
            Change::TableRemoved { qual } => {
                let _ = writeln!(out, "- table {qual}");
            }
            Change::ColumnAdded { table, column } => {
                let _ = writeln!(out, "+ {table}.{} {}", column.name, render_col(column));
            }
            Change::ColumnRemoved { table, name } => {
                let _ = writeln!(out, "- {table}.{name}");
            }
            Change::ColumnChanged { table, name, before, after } => {
                let _ = writeln!(
                    out,
                    "~ {table}.{name} {} → {}",
                    render_col(before),
                    render_col(after)
                );
            }
            Change::PrimaryKeyAdded { table, .. } => {
                let _ = writeln!(out, "+ {table} primary key");
            }
            Change::PrimaryKeyRemoved { table, .. } => {
                let _ = writeln!(out, "- {table} primary key");
            }
            Change::PrimaryKeyChanged { table, before, after } => {
                let _ = writeln!(out, "~ {table} primary key");
                let _ = writeln!(out, "    - {}", before.definition);
                let _ = writeln!(out, "    + {}", after.definition);
            }
            Change::IndexAdded { table, name, index } => {
                let _ = writeln!(out, "+ {table} index {name}");
                let _ = writeln!(out, "    {}", index.definition);
            }
            Change::IndexRemoved { table, name } => {
                let _ = writeln!(out, "- {table} index {name}");
            }
            Change::IndexChanged { table, name, before, after } => {
                let _ = writeln!(out, "~ {table} index {name}");
                let _ = writeln!(out, "    - {}", before.definition);
                let _ = writeln!(out, "    + {}", after.definition);
            }
            Change::ConstraintAdded { table, name, constraint } => {
                let _ = writeln!(out, "+ {table} constraint {name} ({})", constraint.kind);
                let _ = writeln!(out, "    {}", constraint.definition);
            }
            Change::ConstraintRemoved { table, name } => {
                let _ = writeln!(out, "- {table} constraint {name}");
            }
            Change::ConstraintChanged { table, name, before, after } => {
                let _ = writeln!(out, "~ {table} constraint {name}");
                let _ = writeln!(out, "    - {}", before.definition);
                let _ = writeln!(out, "    + {}", after.definition);
            }
            Change::RlsEnabled { table } => {
                let _ = writeln!(out, "+ {table} row-level security");
            }
            Change::RlsDisabled { table } => {
                let _ = writeln!(out, "- {table} row-level security");
            }
            Change::ViewAdded { qual, materialized, .. } => {
                let _ = writeln!(out, "+ {} {qual}", view_kind(*materialized));
            }
            Change::ViewRemoved { qual, materialized, .. } => {
                let _ = writeln!(out, "- {} {qual}", view_kind(*materialized));
            }
            Change::ViewChanged { qual, materialized, .. } => {
                let _ = writeln!(out, "~ {} {qual}", view_kind(*materialized));
            }
            Change::SequenceAdded { qual, sequence } => {
                let _ = writeln!(out, "+ sequence {qual} {}", sequence.data_type);
            }
            Change::SequenceRemoved { qual } => {
                let _ = writeln!(out, "- sequence {qual}");
            }
            Change::SequenceChanged { qual, .. } => {
                let _ = writeln!(out, "~ sequence {qual}");
            }
            Change::TypeAdded { qual, .. } => {
                let _ = writeln!(out, "+ type {qual}");
            }
            Change::TypeRemoved { qual } => {
                let _ = writeln!(out, "- type {qual}");
            }
            Change::TypeChanged { qual, .. } => {
                let _ = writeln!(out, "~ type {qual}");
            }
            Change::TriggerAdded { table, name, .. } => {
                let _ = writeln!(out, "+ {table} trigger {name}");
            }
            Change::TriggerRemoved { table, name } => {
                let _ = writeln!(out, "- {table} trigger {name}");
            }
            Change::TriggerChanged { table, name, .. } => {
                let _ = writeln!(out, "~ {table} trigger {name}");
            }
            Change::PolicyAdded { table, name, .. } => {
                let _ = writeln!(out, "+ {table} policy {name}");
            }
            Change::PolicyRemoved { table, name } => {
                let _ = writeln!(out, "- {table} policy {name}");
            }
            Change::PolicyChanged { table, name, .. } => {
                let _ = writeln!(out, "~ {table} policy {name}");
            }
            Change::FunctionAdded { qual, .. } => {
                let _ = writeln!(out, "+ function {qual}");
            }
            Change::FunctionRemoved { qual } => {
                let _ = writeln!(out, "- function {qual}");
            }
            Change::FunctionChanged { qual, .. } => {
                let _ = writeln!(out, "~ function {qual}");
            }
            Change::ExtensionAdded { name, extension } => {
                let _ = writeln!(out, "+ extension {name} {}", extension.version);
            }
            Change::ExtensionRemoved { name } => {
                let _ = writeln!(out, "- extension {name}");
            }
            Change::ExtensionChanged { name, before, after } => {
                let _ = writeln!(out, "~ extension {name} {} → {}", before.version, after.version);
            }
            Change::PartitionByChanged { table, before, after } => {
                let _ = writeln!(out, "~ {table} partition by");
                let _ = writeln!(out, "    - {}", before.as_ref().map(|p| format!("{} {}", p.strategy, p.key)).unwrap_or_else(|| "(none)".into()));
                let _ = writeln!(out, "    + {}", after.as_ref().map(|p| format!("{} {}", p.strategy, p.key)).unwrap_or_else(|| "(none)".into()));
            }
            Change::PartitionOfChanged { table, before, after } => {
                let _ = writeln!(out, "~ {table} partition of");
                let _ = writeln!(out, "    - {}", before.as_ref().map(|p| format!("{} {}", p.parent, p.bound)).unwrap_or_else(|| "(none)".into()));
                let _ = writeln!(out, "    + {}", after.as_ref().map(|p| format!("{} {}", p.parent, p.bound)).unwrap_or_else(|| "(none)".into()));
            }
        }
    }
    out
}

fn view_kind(materialized: bool) -> &'static str {
    if materialized { "matview" } else { "view" }
}

fn render_col(c: &Column) -> String {
    let null = if c.nullable { "NULL" } else { "NOT NULL" };
    let mut s = format!("{} {}", c.data_type, null);
    if let Some(d) = &c.default {
        s.push_str(&format!(" DEFAULT {d}"));
    }
    if c.identity.is_some() {
        s.push_str(" IDENTITY");
    }
    if let Some(g) = &c.generated {
        s.push_str(&format!(" GENERATED ({g})"));
    }
    s
}

pub fn json(changes: &[Change]) -> Result<String> {
    Ok(serde_json::to_string_pretty(changes)?)
}
