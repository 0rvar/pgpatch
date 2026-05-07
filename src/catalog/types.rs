use crate::model::UserType;
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub fn fetch(client: &mut Client, schema_oid: u32) -> Result<BTreeMap<String, UserType>> {
    // typtype: e=enum, c=composite, d=domain, r=range. Skip implicit row types
    // and array types — those are derived from tables/scalars and re-emit
    // automatically when the source object is recreated.
    let rows = client
        .query(
            "SELECT t.oid, t.typname, t.typtype, t.typrelid, t.typbasetype \
             FROM pg_type t \
             WHERE t.typnamespace = $1 \
               AND t.typtype IN ('e', 'c', 'd', 'r') \
               AND NOT EXISTS ( \
                 SELECT 1 FROM pg_class c \
                 WHERE c.oid = t.typrelid AND c.relkind <> 'c' \
               ) \
               AND NOT EXISTS ( \
                 SELECT 1 FROM pg_depend d \
                 WHERE d.objid = t.oid AND d.deptype = 'a' \
               ) \
             ORDER BY t.typname",
            &[&schema_oid],
        )
        .context("listing pg_type")?;

    let mut out = BTreeMap::new();
    for r in rows {
        let oid: u32 = r.get("oid");
        let name: String = r.get("typname");
        let typtype: i8 = r.get("typtype");
        let typrelid: u32 = r.get("typrelid");
        let typbasetype: u32 = r.get("typbasetype");

        let user_type = match typtype as u8 {
            b'e' => fetch_enum(client, oid)?,
            b'c' => fetch_composite(client, typrelid)?,
            b'd' => fetch_domain(client, oid, typbasetype)?,
            b'r' => fetch_range(client, oid)?,
            _ => continue,
        };
        out.insert(name, user_type);
    }
    Ok(out)
}

fn fetch_enum(client: &mut Client, type_oid: u32) -> Result<UserType> {
    let rows = client
        .query(
            "SELECT enumlabel FROM pg_enum \
             WHERE enumtypid = $1 ORDER BY enumsortorder",
            &[&type_oid],
        )
        .context("listing pg_enum")?;
    let values = rows.into_iter().map(|r| r.get::<_, String>("enumlabel")).collect();
    Ok(UserType::Enum { values })
}

fn fetch_composite(client: &mut Client, typrelid: u32) -> Result<UserType> {
    let rows = client
        .query(
            "SELECT a.attname, format_type(a.atttypid, a.atttypmod) AS data_type \
             FROM pg_attribute a \
             WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
            &[&typrelid],
        )
        .context("listing composite fields")?;
    let fields = rows
        .into_iter()
        .map(|r| (r.get::<_, String>("attname"), r.get::<_, String>("data_type")))
        .collect();
    Ok(UserType::Composite { fields })
}

fn fetch_domain(client: &mut Client, type_oid: u32, base_oid: u32) -> Result<UserType> {
    let base_row = client
        .query_one(
            "SELECT format_type($1, NULL) AS base_type",
            &[&base_oid],
        )
        .context("formatting domain base type")?;
    let base_type: String = base_row.get("base_type");

    let constraints = client
        .query(
            "SELECT pg_get_constraintdef(oid, true) AS def \
             FROM pg_constraint WHERE contypid = $1 ORDER BY conname",
            &[&type_oid],
        )
        .context("listing domain constraints")?;
    let parts: Vec<String> = constraints
        .into_iter()
        .map(|r| r.get::<_, String>("def"))
        .collect();
    let definition = parts.join(" ");
    Ok(UserType::Domain { base_type, definition })
}

fn fetch_range(client: &mut Client, type_oid: u32) -> Result<UserType> {
    let row = client
        .query_one(
            "SELECT format_type(rngsubtype, NULL) AS subtype \
             FROM pg_range WHERE rngtypid = $1",
            &[&type_oid],
        )
        .context("looking up pg_range")?;
    let subtype: String = row.get("subtype");
    Ok(UserType::Range { subtype: subtype.clone(), definition: subtype })
}
