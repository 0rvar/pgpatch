// Diffs on partitioned tables. A partition inherits every column from its
// parent, so a change on the parent shows up on each partition too; the diff
// must emit it once, against the parent, because Postgres rejects the same
// statement on the partition (or has already applied it via recursion).
// Dropping the parent likewise drops its partitions.

use pgpatch::diff::{Change, diff};
use pgpatch::model::{Column, Namespace, PartitionBy, PartitionInfo, Schema, Table};
use std::collections::BTreeMap;

fn col(name: &str, default: Option<&str>) -> Column {
    Column {
        name: name.into(),
        data_type: "text".into(),
        nullable: true,
        default: default.map(Into::into),
        identity: None,
        generated: None,
        collation: None,
        comment: None,
    }
}

fn parent(columns: Vec<Column>) -> Table {
    Table {
        columns,
        partition_by: Some(PartitionBy {
            strategy: "LIST".into(),
            key: "(name)".into(),
        }),
        ..Default::default()
    }
}

fn partition(columns: Vec<Column>) -> Table {
    Table {
        columns,
        partition_of: Some(PartitionInfo {
            parent: "pgboss.job".into(),
            bound: "DEFAULT".into(),
        }),
        ..Default::default()
    }
}

fn schema(job: Table, job_common: Table) -> Schema {
    let mut tables = BTreeMap::new();
    tables.insert("job".to_string(), job);
    tables.insert("job_common".to_string(), job_common);
    let mut schemas = BTreeMap::new();
    schemas.insert(
        "pgboss".to_string(),
        Namespace {
            tables,
            ..Default::default()
        },
    );
    Schema {
        schemas,
        ..Default::default()
    }
}

fn column_changes(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .filter_map(|c| match c {
            Change::ColumnAdded { table, column } => Some(format!("+ {table}.{}", column.name)),
            Change::ColumnRemoved { table, name } => Some(format!("- {table}.{name}")),
            Change::ColumnChanged { table, name, .. } => Some(format!("~ {table}.{name}")),
            _ => None,
        })
        .collect()
}

#[test]
fn dropped_parent_column_is_removed_once_on_the_parent() {
    let before = schema(
        parent(vec![col("name", None), col("blocked", None)]),
        partition(vec![col("name", None), col("blocked", None)]),
    );
    let after = schema(
        parent(vec![col("name", None)]),
        partition(vec![col("name", None)]),
    );

    assert_eq!(
        column_changes(&diff(&before, &after)),
        vec!["- pgboss.job.blocked"]
    );
}

#[test]
fn added_parent_column_is_added_once_on_the_parent() {
    let before = schema(
        parent(vec![col("name", None)]),
        partition(vec![col("name", None)]),
    );
    let after = schema(
        parent(vec![col("name", None), col("blocked", None)]),
        partition(vec![col("name", None), col("blocked", None)]),
    );

    assert_eq!(
        column_changes(&diff(&before, &after)),
        vec!["+ pgboss.job.blocked"]
    );
}

#[test]
fn column_change_mirrored_from_parent_is_emitted_once_on_the_parent() {
    let before = schema(
        parent(vec![col("name", None)]),
        partition(vec![col("name", None)]),
    );
    let after = schema(
        parent(vec![col("name", Some("'x'::text"))]),
        partition(vec![col("name", Some("'x'::text"))]),
    );

    assert_eq!(
        column_changes(&diff(&before, &after)),
        vec!["~ pgboss.job.name"]
    );
}

#[test]
fn partition_local_column_change_is_kept() {
    let before = schema(
        parent(vec![col("name", None)]),
        partition(vec![col("name", None)]),
    );
    let after = schema(
        parent(vec![col("name", None)]),
        partition(vec![col("name", Some("'x'::text"))]),
    );

    assert_eq!(
        column_changes(&diff(&before, &after)),
        vec!["~ pgboss.job_common.name"]
    );
}

#[test]
fn ordinary_tables_are_unaffected() {
    let before = schema(
        Table {
            columns: vec![col("a", None), col("b", None)],
            ..Default::default()
        },
        Table {
            columns: vec![col("a", None), col("b", None)],
            ..Default::default()
        },
    );
    let after = schema(
        Table {
            columns: vec![col("a", None)],
            ..Default::default()
        },
        Table {
            columns: vec![col("a", None)],
            ..Default::default()
        },
    );

    assert_eq!(
        column_changes(&diff(&before, &after)),
        vec!["- pgboss.job.b", "- pgboss.job_common.b"]
    );
}

fn table_changes(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .filter_map(|c| match c {
            Change::TableAdded { qual, .. } => Some(format!("+ {qual}")),
            Change::TableRemoved { qual } => Some(format!("- {qual}")),
            _ => None,
        })
        .collect()
}

fn schema_of(tables: Vec<(&str, Table)>) -> Schema {
    let mut map = BTreeMap::new();
    for (name, table) in tables {
        map.insert(name.to_string(), table);
    }
    let mut schemas = BTreeMap::new();
    schemas.insert(
        "pgboss".to_string(),
        Namespace {
            tables: map,
            ..Default::default()
        },
    );
    Schema {
        schemas,
        ..Default::default()
    }
}

#[test]
fn dropped_parent_takes_its_partitions_with_it() {
    let before = schema(
        parent(vec![col("name", None)]),
        partition(vec![col("name", None)]),
    );
    let after = schema_of(vec![]);

    assert_eq!(table_changes(&diff(&before, &after)), vec!["- pgboss.job"]);
}

#[test]
fn dropping_a_single_partition_keeps_its_removal() {
    let before = schema(
        parent(vec![col("name", None)]),
        partition(vec![col("name", None)]),
    );
    let after = schema_of(vec![("job", parent(vec![col("name", None)]))]);

    assert_eq!(
        table_changes(&diff(&before, &after)),
        vec!["- pgboss.job_common"]
    );
}

#[test]
fn added_parent_and_partition_are_both_created() {
    let before = schema_of(vec![]);
    let after = schema(
        parent(vec![col("name", None)]),
        partition(vec![col("name", None)]),
    );

    assert_eq!(
        table_changes(&diff(&before, &after)),
        vec!["+ pgboss.job", "+ pgboss.job_common"]
    );
}
