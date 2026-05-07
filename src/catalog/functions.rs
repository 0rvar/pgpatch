use crate::model::Function;
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub fn fetch(client: &mut Client, schema_oid: u32) -> Result<BTreeMap<String, Function>> {
    // Skip extension-owned functions (pg_depend deptype='e') — those come back
    // implicitly when the extension is created.
    let rows = client
        .query(
            "SELECT \
                p.proname, \
                pg_get_function_identity_arguments(p.oid) AS args, \
                pg_get_functiondef(p.oid) AS def \
             FROM pg_proc p \
             WHERE p.pronamespace = $1 \
               AND NOT EXISTS ( \
                 SELECT 1 FROM pg_depend d \
                 WHERE d.classid = 'pg_proc'::regclass \
                   AND d.objid = p.oid AND d.deptype = 'e' \
               ) \
             ORDER BY p.proname, args",
            &[&schema_oid],
        )
        .context("listing pg_proc")?;

    let mut out = BTreeMap::new();
    for r in rows {
        let name: String = r.get("proname");
        let args: String = r.get("args");
        let def: String = r.get("def");
        let key = format!("{name}({args})");
        out.insert(key, Function { definition: def });
    }
    Ok(out)
}
