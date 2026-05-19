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
                roles: normalize_roles(r.get("roles")),
                qual: r.get("qual"),
                with_check: r.get("with_check"),
            },
        );
    }
    Ok(out)
}

/// Sort policy role names to a canonical order. Roles in a PG policy are a set
/// — `pg_policy.polroles` preserves the OID insertion order, so two
/// semantically-identical policies declared with their roles in different
/// orders would otherwise compare unequal and produce spurious DROP+CREATE.
pub fn normalize_roles(mut roles: Vec<String>) -> Vec<String> {
    roles.sort();
    roles
}

#[cfg(test)]
mod tests {
    use super::normalize_roles;

    #[test]
    fn normalize_roles_sorts_ascending() {
        let got = normalize_roles(vec!["authenticated".into(), "anon".into()]);
        assert_eq!(got, vec!["anon".to_string(), "authenticated".to_string()]);
    }

    #[test]
    fn normalize_roles_is_idempotent() {
        let once = normalize_roles(vec!["b".into(), "a".into(), "c".into()]);
        let twice = normalize_roles(once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn normalize_roles_preserves_membership() {
        // Same set of roles in different orders must produce equal output.
        let a = normalize_roles(vec!["anon".into(), "authenticated".into()]);
        let b = normalize_roles(vec!["authenticated".into(), "anon".into()]);
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_roles_keeps_public_literal() {
        // PUBLIC sorts above lowercase names — fine, just verify it's preserved.
        let got = normalize_roles(vec!["anon".into(), "PUBLIC".into()]);
        assert_eq!(got, vec!["PUBLIC".to_string(), "anon".to_string()]);
    }

    #[test]
    fn normalize_roles_empty() {
        let got = normalize_roles(Vec::<String>::new());
        assert!(got.is_empty());
    }
}
