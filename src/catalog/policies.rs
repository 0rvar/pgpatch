use crate::model::Policy;
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub fn fetch(client: &mut Client, table_oid: u32) -> Result<BTreeMap<String, Policy>> {
    // polroles is an OID array; 0 means PUBLIC. Resolve real role names so the
    // artefact survives a role rename downstream of the role itself.
    let rows = client
        .query(
            "SELECT \
                p.polname, \
                CASE p.polcmd \
                    WHEN 'r' THEN 'SELECT' \
                    WHEN 'a' THEN 'INSERT' \
                    WHEN 'w' THEN 'UPDATE' \
                    WHEN 'd' THEN 'DELETE' \
                    WHEN '*' THEN 'ALL' \
                END AS command, \
                p.polpermissive, \
                ARRAY( \
                    SELECT CASE WHEN r = 0 THEN 'PUBLIC' \
                                ELSE (SELECT rolname FROM pg_roles WHERE oid = r) END \
                    FROM unnest(p.polroles) r \
                )::text[] AS roles, \
                pg_get_expr(p.polqual, p.polrelid) AS qual, \
                pg_get_expr(p.polwithcheck, p.polrelid) AS with_check \
             FROM pg_policy p \
             WHERE p.polrelid = $1 \
             ORDER BY p.polname",
            &[&table_oid],
        )
        .context("listing pg_policy")?;

    let mut out = BTreeMap::new();
    for r in rows {
        let name: String = r.get("polname");
        out.insert(
            name,
            Policy {
                command: r.get("command"),
                permissive: r.get("polpermissive"),
                roles: r.get("roles"),
                qual: r.get("qual"),
                with_check: r.get("with_check"),
            },
        );
    }
    Ok(out)
}
