use pgpatch::diff::Change;
use pgpatch::emit::sql;
use pgpatch::model::{Column, PartitionBy, PartitionInfo, QualifiedName, Table};

fn col(name: &str, ty: &str, nullable: bool) -> Column {
    Column {
        name: name.into(),
        data_type: ty.into(),
        nullable,
        default: None,
        identity: None,
        generated: None,
        collation: None,
        comment: None,
    }
}

fn qual(s: &str, n: &str) -> QualifiedName {
    QualifiedName::new(s, n)
}

#[test]
fn partitioned_parent_creates_with_partition_by_range() {
    let table = Table {
        columns: vec![col("id", "uuid", false), col("started_at", "timestamptz", false)],
        partition_by: Some(PartitionBy {
            strategy: "RANGE".into(),
            key: "(started_at)".into(),
        }),
        ..Default::default()
    };
    let out = sql(&[Change::TableAdded { qual: qual("public", "memories_partition"), table }]);
    assert!(
        out.contains(") PARTITION BY RANGE (started_at);"),
        "expected trailing PARTITION BY clause; got:\n{out}"
    );
    assert!(out.contains("CREATE TABLE public.memories_partition ("));
}

#[test]
fn partitioned_parent_list_strategy() {
    let table = Table {
        columns: vec![col("name", "text", false)],
        partition_by: Some(PartitionBy {
            strategy: "LIST".into(),
            key: "(name)".into(),
        }),
        ..Default::default()
    };
    let out = sql(&[Change::TableAdded { qual: qual("pgboss", "job"), table }]);
    assert!(out.contains(") PARTITION BY LIST (name);"), "got:\n{out}");
}

#[test]
fn partitioned_parent_hash_strategy_with_multi_col_key() {
    let table = Table {
        columns: vec![col("a", "int", false), col("b", "int", false)],
        partition_by: Some(PartitionBy {
            strategy: "HASH".into(),
            key: "(a, b)".into(),
        }),
        ..Default::default()
    };
    let out = sql(&[Change::TableAdded { qual: qual("public", "h"), table }]);
    assert!(out.contains(") PARTITION BY HASH (a, b);"), "got:\n{out}");
}

#[test]
fn partition_child_uses_short_partition_of_form() {
    let table = Table {
        columns: vec![col("name", "text", false)],
        partition_of: Some(PartitionInfo {
            parent: "pgboss.job".into(),
            bound: "DEFAULT".into(),
        }),
        ..Default::default()
    };
    let out = sql(&[Change::TableAdded { qual: qual("pgboss", "job_common"), table }]);
    assert!(
        out.contains("CREATE TABLE pgboss.job_common PARTITION OF pgboss.job DEFAULT;"),
        "expected DEFAULT partition; got:\n{out}"
    );
    // Children must NOT redeclare columns — column copying happens via the
    // parent automatically.
    assert!(!out.contains("name text"), "child must not redeclare columns; got:\n{out}");
}

#[test]
fn partition_child_range_bound_round_trips() {
    let table = Table {
        partition_of: Some(PartitionInfo {
            parent: "public.memories_partition".into(),
            bound: "FOR VALUES FROM ('2026-01-01') TO ('2026-02-01')".into(),
        }),
        ..Default::default()
    };
    let out = sql(&[Change::TableAdded { qual: qual("partitioned", "memories_2026_01"), table }]);
    assert!(out.contains(
        "CREATE TABLE partitioned.memories_2026_01 PARTITION OF public.memories_partition FOR VALUES FROM ('2026-01-01') TO ('2026-02-01');"
    ), "got:\n{out}");
}

#[test]
fn partition_child_list_bound_round_trips() {
    let table = Table {
        partition_of: Some(PartitionInfo {
            parent: "pgboss.job".into(),
            bound: "FOR VALUES IN ('q1', 'q2')".into(),
        }),
        ..Default::default()
    };
    let out = sql(&[Change::TableAdded { qual: qual("pgboss", "job_q1"), table }]);
    assert!(out.contains(
        "CREATE TABLE pgboss.job_q1 PARTITION OF pgboss.job FOR VALUES IN ('q1', 'q2');"
    ), "got:\n{out}");
}

#[test]
fn partition_attach_emitted_when_partition_of_added() {
    let out = sql(&[Change::PartitionOfChanged {
        table: qual("pgboss", "job_common"),
        before: None,
        after: Some(PartitionInfo {
            parent: "pgboss.job".into(),
            bound: "DEFAULT".into(),
        }),
    }]);
    assert!(
        out.contains("ALTER TABLE pgboss.job ATTACH PARTITION pgboss.job_common DEFAULT;"),
        "got:\n{out}"
    );
}

#[test]
fn partition_detach_emitted_when_partition_of_removed() {
    let out = sql(&[Change::PartitionOfChanged {
        table: qual("pgboss", "job_q1"),
        before: Some(PartitionInfo {
            parent: "pgboss.job".into(),
            bound: "FOR VALUES IN ('q1')".into(),
        }),
        after: None,
    }]);
    assert!(
        out.contains("ALTER TABLE pgboss.job DETACH PARTITION pgboss.job_q1;"),
        "got:\n{out}"
    );
}

#[test]
fn partition_bound_change_emits_detach_then_attach() {
    let out = sql(&[Change::PartitionOfChanged {
        table: qual("public", "p1"),
        before: Some(PartitionInfo {
            parent: "public.parent".into(),
            bound: "FOR VALUES FROM (0) TO (100)".into(),
        }),
        after: Some(PartitionInfo {
            parent: "public.parent".into(),
            bound: "FOR VALUES FROM (0) TO (200)".into(),
        }),
    }]);
    let detach = out
        .find("DETACH PARTITION public.p1")
        .expect("expected detach line");
    let attach = out
        .find("ATTACH PARTITION public.p1 FOR VALUES FROM (0) TO (200)")
        .expect("expected attach line with new bound");
    assert!(detach < attach, "detach must precede attach; got:\n{out}");
}

#[test]
fn partition_by_change_emits_todo_only() {
    let out = sql(&[Change::PartitionByChanged {
        table: qual("public", "t"),
        before: Some(PartitionBy { strategy: "RANGE".into(), key: "(a)".into() }),
        after: Some(PartitionBy { strategy: "LIST".into(), key: "(a)".into() }),
    }]);
    assert!(out.contains("-- TODO: PARTITION BY clause changed on public.t"), "got:\n{out}");
    // No raw ALTER TABLE … PARTITION BY exists — make sure we don't pretend.
    assert!(!out.contains("ALTER TABLE public.t PARTITION BY"), "got:\n{out}");
}

#[test]
fn partitioned_parent_with_primary_key_includes_pk_inside_create() {
    use pgpatch::model::Index;
    let table = Table {
        columns: vec![col("id", "uuid", false), col("started_at", "timestamptz", false)],
        primary_key: Some(Index {
            definition: "PRIMARY KEY (id, started_at)".into(),
            unique: true,
            primary: true,
        }),
        partition_by: Some(PartitionBy { strategy: "RANGE".into(), key: "(started_at)".into() }),
        ..Default::default()
    };
    let out = sql(&[Change::TableAdded { qual: qual("public", "p"), table }]);
    // The PARTITION BY clause must come after the closing paren of the column
    // list, with the inline PK still inside.
    let create_idx = out.find("CREATE TABLE public.p (").unwrap();
    let pk_idx = out[create_idx..].find("PRIMARY KEY (id, started_at)").map(|i| create_idx + i).unwrap();
    let partby_idx = out[create_idx..]
        .find(") PARTITION BY RANGE (started_at)")
        .map(|i| create_idx + i)
        .unwrap();
    assert!(pk_idx < partby_idx, "PK must be inside CREATE; got:\n{out}");
}

#[test]
fn child_with_indexes_emits_create_partition_then_indexes() {
    use pgpatch::model::Index;
    let mut indexes = std::collections::BTreeMap::new();
    indexes.insert(
        "job_common_extra_idx".into(),
        Index {
            definition: "CREATE INDEX job_common_extra_idx ON pgboss.job_common (id)".into(),
            unique: false,
            primary: false,
        },
    );
    let table = Table {
        partition_of: Some(PartitionInfo { parent: "pgboss.job".into(), bound: "DEFAULT".into() }),
        indexes,
        ..Default::default()
    };
    let out = sql(&[Change::TableAdded { qual: qual("pgboss", "job_common"), table }]);
    let create_idx = out
        .find("CREATE TABLE pgboss.job_common PARTITION OF pgboss.job DEFAULT;")
        .expect("partition-of create missing");
    let index_idx = out
        .find("CREATE INDEX job_common_extra_idx")
        .expect("child index missing");
    assert!(create_idx < index_idx, "CREATE TABLE must precede CREATE INDEX; got:\n{out}");
}
