use crate::model::View;
use anyhow::{Context, Result};
use postgres::Client;
use std::collections::BTreeMap;

pub struct ViewBuckets {
    pub views: BTreeMap<String, View>,
    pub materialized_views: BTreeMap<String, View>,
}

pub fn fetch(client: &mut Client, schema_oid: u32) -> Result<ViewBuckets> {
    // reloptions is a text[] of "key=value" items — split it so the artefact
    // captures things like security_invoker=true that pg_get_viewdef strips.
    //
    // pg_depend resolves the relations referenced by the view's rewrite rule
    // (`d.classid = 'pg_rewrite'::regclass`). Distinct because a view can
    // reference the same target through multiple columns. Self-references
    // are filtered out — pg_depend reports the view as depending on itself
    // through its rewrite rule which we don't want in the topo graph.
    let rows = client
        .query(
            "SELECT \
                c.oid, \
                c.relname, \
                c.relkind, \
                pg_get_viewdef(c.oid, true) AS def, \
                COALESCE(c.reloptions, ARRAY[]::text[]) AS opts, \
                COALESCE( \
                    (SELECT array_agg(DISTINCT dn.nspname || '.' || dc.relname) \
                     FROM pg_depend d \
                     JOIN pg_rewrite rw ON rw.oid = d.objid \
                     JOIN pg_class dc ON dc.oid = d.refobjid \
                     JOIN pg_namespace dn ON dn.oid = dc.relnamespace \
                     WHERE d.classid = 'pg_rewrite'::regclass \
                       AND d.refclassid = 'pg_class'::regclass \
                       AND rw.ev_class = c.oid \
                       AND d.refobjid <> c.oid), \
                    ARRAY[]::text[] \
                ) AS depends_on \
             FROM pg_class c \
             WHERE c.relnamespace = $1 \
               AND c.relkind IN ('v', 'm') \
             ORDER BY c.relname",
            &[&schema_oid],
        )
        .context("listing views")?;

    let mut views = BTreeMap::new();
    let mut materialized_views = BTreeMap::new();
    for r in rows {
        let name: String = r.get("relname");
        let kind: i8 = r.get("relkind");
        let def: String = r.get("def");
        let opts: Vec<String> = r.get("opts");
        let mut depends_on: Vec<String> = r.get("depends_on");
        depends_on.sort();

        let mut options = BTreeMap::new();
        for o in opts {
            if let Some((k, v)) = o.split_once('=') {
                options.insert(k.to_string(), v.to_string());
            } else {
                options.insert(o, String::new());
            }
        }

        let view = View { definition: def, options, depends_on };
        if kind as u8 == b'm' {
            materialized_views.insert(name, view);
        } else {
            views.insert(name, view);
        }
    }
    Ok(ViewBuckets { views, materialized_views })
}
