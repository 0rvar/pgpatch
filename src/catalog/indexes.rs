use crate::model::Index;
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub struct Indexes {
    pub primary_key: Option<Index>,
    pub indexes: BTreeMap<String, Index>,
}

pub fn fetch(client: &mut Client, table_oid: u32) -> Result<Indexes> {
    let rows = client
        .query(
            "SELECT \
                ic.relname AS name, \
                i.indisprimary, \
                i.indisunique, \
                pg_get_indexdef(i.indexrelid, 0, true) AS def, \
                c.conname IS NOT NULL AS constraint_backed \
             FROM pg_index i \
             JOIN pg_class ic ON ic.oid = i.indexrelid \
             LEFT JOIN pg_constraint c ON c.conindid = i.indexrelid \
             WHERE i.indrelid = $1 \
               AND NOT EXISTS ( \
                 SELECT 1 FROM pg_inherits h WHERE h.inhrelid = i.indexrelid \
               ) \
             ORDER BY ic.relname",
            &[&table_oid],
        )
        .context("listing pg_index")?;

    let mut primary_key = None;
    let mut indexes = BTreeMap::new();
    for r in rows {
        let name: String = r.get("name");
        let primary: bool = r.get("indisprimary");
        let unique: bool = r.get("indisunique");
        let def: String = r.get("def");
        let constraint_backed: bool = r.get("constraint_backed");
        // On a partitioned table pg_get_indexdef says `ON ONLY`, which would
        // recreate the parent alone (invalid, with no partition indexes).
        // Plain `ON` makes CREATE INDEX cascade to every partition and land
        // valid. Attached partition indexes are excluded by the query above
        // (they are created and dropped through the parent), so ON ONLY is
        // the only partition-specific wrinkle left in the definition.
        let def = def.replacen(" ON ONLY ", " ON ", 1);
        let idx = Index {
            definition: def,
            unique,
            primary,
        };

        if primary {
            primary_key = Some(idx);
        } else if !constraint_backed {
            // Skip indexes that are owned by a constraint (UNIQUE / EXCLUDE / PK) —
            // they're already represented under `constraints`.
            indexes.insert(name, idx);
        }
    }
    Ok(Indexes {
        primary_key,
        indexes,
    })
}
