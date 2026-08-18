use crate::model::{Function, GrantEntry};
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub fn fetch(
    client: &mut Client,
    schema_oid: u32,
    ignore_grants: bool,
) -> Result<BTreeMap<String, Function>> {
    // Skip extension-owned functions (pg_depend deptype='e') — those come back
    // implicitly when the extension is created.
    //
    // The LEFT JOIN LATERAL over aclexplode yields one row per ACL entry, or
    // a single all-NULL row when proacl is NULL (default ACL) or empty
    // (everything revoked). `acl_is_default` disambiguates the two: NULL
    // proacl means "unmanaged", an empty array is a real fully-revoked ACL.
    // The PUBLIC pseudo-role is identified by grantee oid 0, never by role
    // name — a real role spelled "PUBLIC" must not collapse into the keyword.
    let rows = client
        .query(
            "SELECT \
                p.proname, \
                pg_get_function_identity_arguments(p.oid) AS args, \
                pg_get_functiondef(p.oid) AS def, \
                COALESCE(pg_get_function_result(p.oid), '') AS result_type, \
                p.proacl IS NULL AS acl_is_default, \
                e.grantee_is_public, \
                e.grantee_role, \
                e.privilege, \
                e.grantable \
             FROM pg_proc p \
             LEFT JOIN LATERAL ( \
                SELECT a.grantee = 0 AS grantee_is_public, \
                       r.rolname AS grantee_role, \
                       a.privilege_type AS privilege, \
                       a.is_grantable AS grantable \
                FROM aclexplode(p.proacl) a \
                LEFT JOIN pg_roles r ON r.oid = a.grantee \
             ) e ON true \
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

    let mut out: BTreeMap<String, Function> = BTreeMap::new();
    for r in rows {
        let name: String = r.get("proname");
        let args: String = r.get("args");
        let key = format!("{name}({args})");
        let func = out.entry(key).or_insert_with(|| Function {
            definition: r.get("def"),
            result_type: Some(r.get("result_type")),
            name: name.clone(),
            identity_args: args.clone(),
            acl: if ignore_grants || r.get::<_, bool>("acl_is_default") {
                None
            } else {
                Some(Vec::new())
            },
        });
        if let Some(acl) = &mut func.acl {
            // `grantee_is_public` is NULL exactly when the lateral produced
            // the padding all-NULL row (no ACL entries at all).
            if let Some(is_public) = r.get::<_, Option<bool>>("grantee_is_public") {
                acl.push(GrantEntry {
                    grantee: if is_public { None } else { r.get("grantee_role") },
                    privilege: r.get("privilege"),
                    grantable: r.get("grantable"),
                });
            }
        }
    }
    for func in out.values_mut() {
        if let Some(acl) = &mut func.acl {
            // aclexplode yields one row per (grantor, grantee, privilege);
            // the grantor is dropped here, so the same effective grant held
            // from two grantors would otherwise appear twice and flag a
            // perpetual phantom grants diff.
            acl.sort();
            acl.dedup();
        }
    }
    Ok(out)
}
