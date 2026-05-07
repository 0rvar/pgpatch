use crate::model::Constraint;
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub fn fetch(client: &mut Client, table_oid: u32) -> Result<BTreeMap<String, Constraint>> {
    // contype: p=primary, f=foreign key, u=unique, c=check, x=exclusion, t=trigger.
    // Primary keys and uniques are also indexes — we keep them as constraints
    // here and let the index path skip the matching index by name.
    let rows = client
        .query(
            "SELECT conname, contype, pg_get_constraintdef(oid, true) AS def \
             FROM pg_constraint \
             WHERE conrelid = $1 \
             ORDER BY conname",
            &[&table_oid],
        )
        .context("listing pg_constraint")?;

    let mut out = BTreeMap::new();
    for r in rows {
        let name: String = r.get("conname");
        let contype: i8 = r.get("contype");
        let def: String = r.get("def");
        let kind = match contype as u8 {
            b'p' => "primary_key",
            b'f' => "foreign_key",
            b'u' => "unique",
            b'c' => "check",
            b'x' => "exclusion",
            b't' => "trigger",
            other => {
                // Unknown contype — keep going but tag it so it shows up in artefacts.
                let _ = other;
                "other"
            }
        }
        .to_string();
        out.insert(name, Constraint { kind, definition: def });
    }
    Ok(out)
}
