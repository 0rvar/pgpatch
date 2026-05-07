pub mod constraints;
pub mod extensions;
pub mod functions;
pub mod indexes;
pub mod policies;
pub mod schemas;
pub mod sequences;
pub mod tables;
pub mod triggers;
pub mod types;
pub mod views;

use crate::config::Config;
use crate::model::{Namespace, Schema, Table};
use crate::tls;
use anyhow::{Context, Result};
use globset::GlobSet;
use postgres::Client;

pub fn snapshot(connection: &str, config: &Config) -> Result<Schema> {
    let mut client = Client::connect(connection, tls::connector())
        .context("connecting to postgres")?;

    // Force pg_get_*def() to fully-qualify every reference. With a default
    // search_path (e.g. "public" or anything that includes auth), the catalog
    // formatters strip the schema for relations/functions that resolve under
    // it — producing snapshots that differ between databases purely on
    // search_path, not on real schema content.
    client
        .batch_execute("SET search_path TO pg_catalog")
        .context("pinning search_path")?;

    let exclude_tables = Config::build_exclude_set(&config.exclude.tables)?;
    let exclude_views = Config::build_exclude_set(&config.exclude.views)?;
    let exclude_functions = Config::build_exclude_set(&config.exclude.functions)?;
    let exclude_extensions = Config::build_exclude_set(&config.exclude.extensions)?;
    let ignore_partitions = Config::build_exclude_set(&config.options.ignore_partitions)?;

    let namespaces = schemas::fetch(&mut client, &config.include.schemas)?;

    let mut schema = Schema::default();
    schema.extensions = extensions::fetch(&mut client)?;
    schema
        .extensions
        .retain(|name, _| !exclude_extensions.is_match(name));
    for ns in namespaces {
        let namespace = build_namespace(
            &mut client,
            &ns,
            &exclude_tables,
            &exclude_views,
            &exclude_functions,
            &ignore_partitions,
        )?;
        schema.schemas.insert(ns.name, namespace);
    }
    Ok(schema)
}

fn build_namespace(
    client: &mut Client,
    ns: &schemas::NamespaceRow,
    exclude_tables: &GlobSet,
    exclude_views: &GlobSet,
    exclude_functions: &GlobSet,
    ignore_partitions: &GlobSet,
) -> Result<Namespace> {
    let mut namespace = Namespace::default();

    let table_rows = tables::fetch_tables(client, ns.oid)?;
    for tr in table_rows {
        let qualified = format!("{}.{}", ns.name, tr.name);
        if exclude_tables.is_match(&qualified) {
            continue;
        }
        if tr.is_partition && ignore_partitions.is_match(&qualified) {
            continue;
        }

        let columns = tables::fetch_columns(client, tr.oid)?;
        let constraint_map = constraints::fetch(client, tr.oid)?;
        let idx = indexes::fetch(client, tr.oid)?;
        let trigger_map = triggers::fetch(client, tr.oid)?;
        let policy_map = policies::fetch(client, tr.oid)?;

        let table = Table {
            columns,
            primary_key: idx.primary_key,
            indexes: idx.indexes,
            constraints: constraint_map,
            triggers: trigger_map,
            policies: policy_map,
            rls_enabled: tr.rls_enabled,
            partition_by: tr.partition_by,
            partition_of: tr.partition_of,
        };
        namespace.tables.insert(tr.name, table);
    }

    let view_buckets = views::fetch(client, ns.oid)?;
    for (name, view) in view_buckets.views {
        let qualified = format!("{}.{}", ns.name, name);
        if !exclude_views.is_match(&qualified) {
            namespace.views.insert(name, view);
        }
    }
    for (name, view) in view_buckets.materialized_views {
        let qualified = format!("{}.{}", ns.name, name);
        if !exclude_views.is_match(&qualified) {
            namespace.materialized_views.insert(name, view);
        }
    }

    namespace.sequences = sequences::fetch(client, ns.oid)?;
    namespace.types = types::fetch(client, ns.oid)?;

    let function_map = functions::fetch(client, ns.oid)?;
    for (key, func) in function_map {
        let qualified = format!("{}.{}", ns.name, key);
        if !exclude_functions.is_match(&qualified) {
            namespace.functions.insert(key, func);
        }
    }

    Ok(namespace)
}
