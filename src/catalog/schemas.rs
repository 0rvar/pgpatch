use anyhow::{Context, Result};
use postgres::Client;

#[derive(Debug)]
pub struct NamespaceRow {
    pub oid: u32,
    pub name: String,
}

pub fn fetch(client: &mut Client, included: &[String]) -> Result<Vec<NamespaceRow>> {
    let rows = client
        .query(
            "SELECT oid, nspname FROM pg_namespace \
             WHERE nspname = ANY($1) \
             ORDER BY nspname",
            &[&included],
        )
        .context("listing pg_namespace")?;
    Ok(rows
        .into_iter()
        .map(|r| NamespaceRow { oid: r.get("oid"), name: r.get("nspname") })
        .collect())
}
