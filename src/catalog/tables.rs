use crate::model::{Column, Identity, PartitionBy, PartitionInfo};
use anyhow::{Context, Result};
use postgres::Client;

#[derive(Debug)]
pub struct TableRow {
    pub oid: u32,
    pub name: String,
    pub rls_enabled: bool,
    pub kind: char, // 'r' = ordinary, 'p' = partitioned parent
    pub is_partition: bool,
    pub partition_by: Option<PartitionBy>,
    pub partition_of: Option<PartitionInfo>,
}

pub fn fetch_tables(client: &mut Client, schema_oid: u32) -> Result<Vec<TableRow>> {
    // partkeydef and partbound are only meaningful for partitioned parents and
    // partition children respectively; the LEFT JOINs leave them NULL otherwise.
    let rows = client
        .query(
            "SELECT \
                c.oid, \
                c.relname, \
                c.relrowsecurity, \
                c.relkind, \
                c.relispartition, \
                pt.partstrat, \
                pg_get_partkeydef(c.oid) AS partkeydef, \
                parent_ns.nspname AS parent_schema, \
                parent.relname AS parent_name, \
                pg_get_expr(c.relpartbound, c.oid) AS partbound \
             FROM pg_class c \
             LEFT JOIN pg_partitioned_table pt ON pt.partrelid = c.oid \
             LEFT JOIN pg_inherits inh ON inh.inhrelid = c.oid AND c.relispartition \
             LEFT JOIN pg_class parent ON parent.oid = inh.inhparent \
             LEFT JOIN pg_namespace parent_ns ON parent_ns.oid = parent.relnamespace \
             WHERE c.relnamespace = $1 \
               AND c.relkind IN ('r', 'p') \
             ORDER BY c.relname",
            &[&schema_oid],
        )
        .context("listing pg_class tables")?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let kind: i8 = r.get("relkind");
        let kind = kind as u8 as char;
        let is_partition: bool = r.get("relispartition");

        let partition_by = if kind == 'p' {
            let strat: Option<i8> = r.get("partstrat");
            let key_def: Option<String> = r.get("partkeydef");
            match (strat, key_def) {
                (Some(s), Some(def)) => Some(PartitionBy {
                    strategy: strategy_label(s as u8 as char).to_string(),
                    key: extract_key_clause(&def),
                }),
                _ => None,
            }
        } else {
            None
        };

        let partition_of = if is_partition {
            let parent_schema: Option<String> = r.get("parent_schema");
            let parent_name: Option<String> = r.get("parent_name");
            let bound: Option<String> = r.get("partbound");
            match (parent_schema, parent_name, bound) {
                (Some(ns), Some(n), Some(b)) => Some(PartitionInfo {
                    parent: format!("{ns}.{n}"),
                    bound: b,
                }),
                _ => None,
            }
        } else {
            None
        };

        out.push(TableRow {
            oid: r.get("oid"),
            name: r.get("relname"),
            rls_enabled: r.get("relrowsecurity"),
            kind,
            is_partition,
            partition_by,
            partition_of,
        });
    }
    Ok(out)
}

pub fn fetch_columns(client: &mut Client, table_oid: u32) -> Result<Vec<Column>> {
    let rows = client
        .query(
            "SELECT \
                a.attname, \
                format_type(a.atttypid, a.atttypmod) AS data_type, \
                NOT a.attnotnull AS nullable, \
                pg_get_expr(d.adbin, d.adrelid) AS default_expr, \
                a.attidentity, \
                a.attgenerated, \
                col_description(a.attrelid, a.attnum) AS comment \
             FROM pg_attribute a \
             LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
            &[&table_oid],
        )
        .context("listing columns")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let attidentity: i8 = r.get("attidentity");
        let attgenerated: i8 = r.get("attgenerated");
        let expr: Option<String> = r.get("default_expr");

        let identity = match attidentity {
            x if x == b'a' as i8 => Some(Identity::Always),
            x if x == b'd' as i8 => Some(Identity::ByDefault),
            _ => None,
        };

        let mut column = Column {
            name: r.get("attname"),
            data_type: r.get("data_type"),
            nullable: r.get("nullable"),
            default: None,
            identity,
            generated: None,
            collation: None,
            comment: r.get("comment"),
        };

        // Generated columns store their expression in pg_attrdef under attgenerated='s'.
        // Plain defaults populate `default`; everything else is dropped on the floor.
        if attgenerated == b's' as i8 {
            column.generated = expr;
        } else if column.identity.is_none() {
            column.default = expr;
        }

        out.push(column);
    }
    Ok(out)
}

fn strategy_label(c: char) -> &'static str {
    match c {
        'r' => "RANGE",
        'l' => "LIST",
        'h' => "HASH",
        _ => "RANGE",
    }
}

// `pg_get_partkeydef` returns the full clause with strategy keyword, e.g.
// `RANGE (started_at)` or `LIST (name)`. We want just the parenthesised key,
// so we drop the leading word (and any following whitespace).
fn extract_key_clause(def: &str) -> String {
    let trimmed = def.trim_start();
    match trimmed.find(|c: char| c.is_whitespace()) {
        Some(i) => trimmed[i..].trim_start().to_string(),
        None => trimmed.to_string(),
    }
}
