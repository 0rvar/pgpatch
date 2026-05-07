use crate::model::Trigger;
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub fn fetch(client: &mut Client, table_oid: u32) -> Result<BTreeMap<String, Trigger>> {
    let rows = client
        .query(
            "SELECT t.tgname, pg_get_triggerdef(t.oid, true) AS def \
             FROM pg_trigger t \
             WHERE t.tgrelid = $1 AND NOT t.tgisinternal \
             ORDER BY t.tgname",
            &[&table_oid],
        )
        .context("listing pg_trigger")?;

    let mut out = BTreeMap::new();
    for r in rows {
        let name: String = r.get("tgname");
        let def: String = r.get("def");
        out.insert(name, Trigger { definition: def });
    }
    Ok(out)
}
