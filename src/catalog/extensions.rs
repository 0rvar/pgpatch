use crate::model::Extension;
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub fn fetch(client: &mut Client) -> Result<BTreeMap<String, Extension>> {
    let rows = client
        .query(
            "SELECT e.extname, e.extversion, n.nspname AS schema \
             FROM pg_extension e \
             JOIN pg_namespace n ON n.oid = e.extnamespace \
             ORDER BY e.extname",
            &[],
        )
        .context("listing pg_extension")?;

    let mut out = BTreeMap::new();
    for r in rows {
        let name: String = r.get("extname");
        out.insert(
            name,
            Extension {
                version: r.get("extversion"),
                schema: r.get("schema"),
            },
        );
    }
    Ok(out)
}
