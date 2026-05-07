use crate::model::Sequence;
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub fn fetch(client: &mut Client, schema_oid: u32) -> Result<BTreeMap<String, Sequence>> {
    // pg_sequence stores the configuration; pg_class is needed for the name.
    // The OWNED BY column comes from pg_depend (deptype='a' = automatic).
    let rows = client
        .query(
            "SELECT \
                c.relname, \
                format_type(s.seqtypid, NULL) AS data_type, \
                s.seqstart, s.seqincrement, s.seqmin, s.seqmax, s.seqcache, s.seqcycle, \
                ( \
                  SELECT format('%I.%I.%I', \
                    (SELECT nspname FROM pg_namespace WHERE oid = oc.relnamespace), \
                    oc.relname, \
                    a.attname) \
                  FROM pg_depend d \
                  JOIN pg_class oc ON oc.oid = d.refobjid \
                  JOIN pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid \
                  WHERE d.classid = 'pg_class'::regclass \
                    AND d.objid = c.oid \
                    AND d.deptype = 'a' \
                  LIMIT 1 \
                ) AS owned_by \
             FROM pg_class c \
             JOIN pg_sequence s ON s.seqrelid = c.oid \
             WHERE c.relnamespace = $1 \
             ORDER BY c.relname",
            &[&schema_oid],
        )
        .context("listing sequences")?;

    let mut out = BTreeMap::new();
    for r in rows {
        let name: String = r.get("relname");
        out.insert(
            name,
            Sequence {
                data_type: r.get("data_type"),
                start: r.get("seqstart"),
                increment: r.get("seqincrement"),
                min_value: r.get("seqmin"),
                max_value: r.get("seqmax"),
                cache: r.get("seqcache"),
                cycle: r.get("seqcycle"),
                owned_by: r.get("owned_by"),
            },
        );
    }
    Ok(out)
}
