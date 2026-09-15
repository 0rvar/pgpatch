use crate::model::Trigger;
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub fn fetch(client: &mut Client, table_oid: u32) -> Result<BTreeMap<String, Trigger>> {
    // A trigger on a partitioned parent is cloned onto every partition with
    // tgparentid pointing at the parent's trigger. The clones follow the
    // parent's trigger through create and drop, so only the partition's own
    // triggers (tgparentid = 0) are snapshotted.
    let rows = client
        .query(
            "SELECT t.tgname, pg_get_triggerdef(t.oid, true) AS def \
             FROM pg_trigger t \
             WHERE t.tgrelid = $1 AND NOT t.tgisinternal AND t.tgparentid = 0 \
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
