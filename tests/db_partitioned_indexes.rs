// Database-backed tests for index handling on partitioned tables.
//
// These need a live PostgreSQL and are skipped unless
// PGPATCH_TEST_DATABASE_URL is set, e.g.
//   PGPATCH_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:54322/postgres cargo test
// Each test works inside its own schema so tests can run in parallel.

use pgpatch::catalog::snapshot;
use pgpatch::config::{Config, Include};
use pgpatch::model::Schema;
use pgpatch::{diff, emit};
use postgres::{Client, NoTls};

fn db_url() -> Option<String> {
    match std::env::var("PGPATCH_TEST_DATABASE_URL") {
        Ok(url) if !url.is_empty() => Some(url),
        _ => {
            eprintln!("PGPATCH_TEST_DATABASE_URL not set; skipping database test");
            None
        }
    }
}

struct TestSchema {
    client: Client,
    url: String,
    name: String,
}

impl TestSchema {
    fn new(name: &str) -> Option<Self> {
        let url = db_url()?;
        let mut client = Client::connect(&url, NoTls).expect("connect");
        client
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name};"
            ))
            .expect("create schema");
        Some(Self {
            client,
            url,
            name: name.to_string(),
        })
    }

    fn exec(&mut self, sql: &str) {
        self.client
            .batch_execute(sql)
            .unwrap_or_else(|e| panic!("{sql}\n{e}"));
    }

    fn snapshot(&self) -> Schema {
        let config = Config {
            include: Include {
                schemas: vec![self.name.clone()],
            },
            exclude: Default::default(),
            options: Default::default(),
        };
        snapshot(&self.url, &config).expect("snapshot")
    }

    fn apply(&mut self, changes: &[diff::Change]) -> Result<(), postgres::Error> {
        let sql = emit::sql(changes);
        let mut tx = self.client.transaction()?;
        tx.batch_execute(&sql)?;
        tx.commit()
    }

    fn index_valid(&mut self, index: &str) -> bool {
        self.client
            .query_one(
                "SELECT i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relname = $2",
                &[&self.name, &index],
            )
            .expect("index exists")
            .get(0)
    }

    fn attached_child_count(&mut self, parent_index: &str) -> i64 {
        self.client
            .query_one(
                "SELECT count(*) FROM pg_inherits h JOIN pg_class p ON p.oid = h.inhparent \
                 JOIN pg_namespace n ON n.oid = p.relnamespace \
                 WHERE n.nspname = $1 AND p.relname = $2",
                &[&self.name, &parent_index],
            )
            .unwrap()
            .get(0)
    }
}

impl Drop for TestSchema {
    fn drop(&mut self) {
        let _ = self
            .client
            .batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.name));
    }
}

const PARTITIONED_TABLE: &str = "\
    CREATE TABLE {s}.job (name text NOT NULL, created_on timestamptz NOT NULL) PARTITION BY LIST (name); \
    CREATE TABLE {s}.job_common PARTITION OF {s}.job DEFAULT;";

fn partitioned_table(schema: &str) -> String {
    PARTITIONED_TABLE.replace("{s}", schema)
}

#[test]
fn parent_index_is_snapshotted_once_without_on_only() {
    let Some(mut db) = TestSchema::new("pgpatch_t_parent_idx") else {
        return;
    };
    db.exec(&partitioned_table("pgpatch_t_parent_idx"));
    db.exec("CREATE INDEX job_created_on_idx ON pgpatch_t_parent_idx.job (created_on);");

    let snap = db.snapshot();
    let ns = &snap.schemas["pgpatch_t_parent_idx"];

    let parent = &ns.tables["job"];
    assert_eq!(
        parent.indexes.len(),
        1,
        "parent indexes: {:?}",
        parent.indexes
    );
    let def = &parent.indexes["job_created_on_idx"].definition;
    assert!(
        !def.contains(" ON ONLY "),
        "definition must cascade to partitions: {def}"
    );
    assert!(
        def.contains(" ON pgpatch_t_parent_idx.job "),
        "unexpected definition: {def}"
    );

    let child = &ns.tables["job_common"];
    assert!(
        child.indexes.is_empty(),
        "attached child indexes belong to the parent: {:?}",
        child.indexes
    );
}

#[test]
fn partition_local_index_is_still_snapshotted() {
    let Some(mut db) = TestSchema::new("pgpatch_t_local_idx") else {
        return;
    };
    db.exec(&partitioned_table("pgpatch_t_local_idx"));
    db.exec("CREATE INDEX job_common_name_idx ON pgpatch_t_local_idx.job_common (name);");

    let snap = db.snapshot();
    let ns = &snap.schemas["pgpatch_t_local_idx"];
    assert!(ns.tables["job"].indexes.is_empty());
    assert_eq!(ns.tables["job_common"].indexes.len(), 1);
    assert!(
        ns.tables["job_common"]
            .indexes
            .contains_key("job_common_name_idx")
    );
}

#[test]
fn parent_index_round_trips_through_drop_and_create() {
    let Some(mut db) = TestSchema::new("pgpatch_t_roundtrip") else {
        return;
    };
    db.exec(&partitioned_table("pgpatch_t_roundtrip"));
    let without_index = db.snapshot();

    db.exec("CREATE INDEX job_created_on_idx ON pgpatch_t_roundtrip.job (created_on);");
    let with_index = db.snapshot();

    // State A (index present) -> B: the parent DROP cascades to the attached
    // child, and the patch must not then try to drop the child on its own.
    let to_without = diff::diff(&with_index, &without_index);
    assert!(!to_without.is_empty());
    db.apply(&to_without)
        .expect("dropping the parent index must succeed");
    assert_eq!(db.snapshot(), without_index);

    // State B -> A: CREATE INDEX on the parent must cascade to the partition
    // so the parent lands valid with an attached child.
    let to_with = diff::diff(&without_index, &with_index);
    assert!(!to_with.is_empty());
    db.apply(&to_with)
        .expect("creating the parent index must succeed");
    assert!(
        db.index_valid("job_created_on_idx"),
        "parent index must be valid"
    );
    assert_eq!(db.attached_child_count("job_created_on_idx"), 1);
    assert_eq!(db.snapshot(), with_index);
}

#[test]
fn dropped_parent_column_round_trips_through_the_partition() {
    let Some(mut db) = TestSchema::new("pgpatch_t_column_drop") else {
        return;
    };
    db.exec(&partitioned_table("pgpatch_t_column_drop"));
    let without_column = db.snapshot();

    db.exec(
        "ALTER TABLE pgpatch_t_column_drop.job ADD COLUMN blocked boolean NOT NULL DEFAULT false;",
    );
    let with_column = db.snapshot();
    assert!(
        with_column.schemas["pgpatch_t_column_drop"].tables["job_common"]
            .columns
            .iter()
            .any(|c| c.name == "blocked")
    );

    // The parent DROP COLUMN cascades into the partition, so the patch must
    // drop the column on the parent only; a second drop on the partition
    // would fail because the column is already gone.
    let to_without = diff::diff(&with_column, &without_column);
    assert!(!to_without.is_empty());
    db.apply(&to_without)
        .expect("dropping the parent column must succeed");
    assert_eq!(db.snapshot(), without_column);

    // ADD COLUMN is likewise only legal on the parent.
    let to_with = diff::diff(&without_column, &with_column);
    assert!(!to_with.is_empty());
    db.apply(&to_with)
        .expect("adding the parent column must succeed");
    assert_eq!(db.snapshot(), with_column);
}

#[test]
fn dropped_parent_table_takes_its_partitions_with_it() {
    let Some(mut db) = TestSchema::new("pgpatch_t_table_drop") else {
        return;
    };
    let empty = db.snapshot();

    db.exec(&partitioned_table("pgpatch_t_table_drop"));
    let with_tables = db.snapshot();

    // DROP TABLE on the parent removes every partition, so the patch must not
    // then drop the partition by name.
    let to_empty = diff::diff(&with_tables, &empty);
    assert!(!to_empty.is_empty());
    db.apply(&to_empty)
        .expect("dropping the parent table must succeed");
    assert_eq!(db.snapshot(), empty);

    let to_with = diff::diff(&empty, &with_tables);
    assert!(!to_with.is_empty());
    db.apply(&to_with)
        .expect("creating parent and partition must succeed");
    assert_eq!(db.snapshot(), with_tables);
}

#[test]
fn partition_local_default_survives_a_parent_default_change() {
    let Some(mut db) = TestSchema::new("pgpatch_t_local_default") else {
        return;
    };
    db.exec(&partitioned_table("pgpatch_t_local_default"));
    let plain = db.snapshot();

    // The parent's SET DEFAULT recurses into the partition; the partition
    // then overrides it. Both states must round-trip, with the partition's
    // own default applied after the parent's.
    db.exec(
        "ALTER TABLE pgpatch_t_local_default.job ALTER COLUMN name SET DEFAULT 'p'; \
         ALTER TABLE pgpatch_t_local_default.job_common ALTER COLUMN name SET DEFAULT 'c';",
    );
    let overridden = db.snapshot();
    let child_default = |snap: &Schema| {
        snap.schemas["pgpatch_t_local_default"].tables["job_common"]
            .columns
            .iter()
            .find(|c| c.name == "name")
            .and_then(|c| c.default.clone())
    };
    assert_eq!(child_default(&overridden).as_deref(), Some("'c'::text"));

    let to_plain = diff::diff(&overridden, &plain);
    assert!(!to_plain.is_empty());
    db.apply(&to_plain)
        .expect("clearing both defaults must succeed");
    assert_eq!(db.snapshot(), plain);

    let to_overridden = diff::diff(&plain, &overridden);
    assert!(!to_overridden.is_empty());
    db.apply(&to_overridden)
        .expect("setting both defaults must succeed");
    assert_eq!(db.snapshot(), overridden);
}

#[test]
fn parent_check_constraint_is_snapshotted_once_and_round_trips() {
    let Some(mut db) = TestSchema::new("pgpatch_t_parent_check") else {
        return;
    };
    db.exec(&partitioned_table("pgpatch_t_parent_check"));
    let without = db.snapshot();

    db.exec(
        "ALTER TABLE pgpatch_t_parent_check.job ADD CONSTRAINT name_not_blank CHECK (name <> '');",
    );
    let with = db.snapshot();
    let ns = &with.schemas["pgpatch_t_parent_check"];
    assert!(ns.tables["job"].constraints.contains_key("name_not_blank"));
    assert!(
        ns.tables["job_common"].constraints.is_empty(),
        "inherited clone must not be snapshotted on the partition: {:?}",
        ns.tables["job_common"].constraints
    );

    let to_without = diff::diff(&with, &without);
    assert!(!to_without.is_empty());
    db.apply(&to_without)
        .expect("dropping the parent constraint must succeed");
    assert_eq!(db.snapshot(), without);

    let to_with = diff::diff(&without, &with);
    assert!(!to_with.is_empty());
    db.apply(&to_with)
        .expect("adding the parent constraint must succeed");
    assert_eq!(db.snapshot(), with);
}

#[test]
fn parent_primary_key_is_snapshotted_once_and_round_trips() {
    let Some(mut db) = TestSchema::new("pgpatch_t_parent_pk") else {
        return;
    };
    db.exec(&partitioned_table("pgpatch_t_parent_pk"));
    let without = db.snapshot();

    db.exec("ALTER TABLE pgpatch_t_parent_pk.job ADD PRIMARY KEY (name, created_on);");
    let with = db.snapshot();
    let ns = &with.schemas["pgpatch_t_parent_pk"];
    assert!(ns.tables["job"].constraints.contains_key("job_pkey"));
    assert!(
        ns.tables["job_common"].constraints.is_empty(),
        "cloned pkey must not be snapshotted on the partition: {:?}",
        ns.tables["job_common"].constraints
    );

    let to_without = diff::diff(&with, &without);
    assert!(!to_without.is_empty());
    db.apply(&to_without)
        .expect("dropping the parent pkey must succeed");
    assert_eq!(db.snapshot(), without);

    let to_with = diff::diff(&without, &with);
    assert!(!to_with.is_empty());
    db.apply(&to_with)
        .expect("adding the parent pkey must succeed");
    assert_eq!(db.snapshot(), with);
}

#[test]
fn partition_local_constraint_is_still_snapshotted() {
    let Some(mut db) = TestSchema::new("pgpatch_t_local_check") else {
        return;
    };
    db.exec(&partitioned_table("pgpatch_t_local_check"));
    db.exec("ALTER TABLE pgpatch_t_local_check.job_common ADD CONSTRAINT local_check CHECK (name <> 'x');");

    let snap = db.snapshot();
    let ns = &snap.schemas["pgpatch_t_local_check"];
    assert!(ns.tables["job"].constraints.is_empty());
    assert!(
        ns.tables["job_common"]
            .constraints
            .contains_key("local_check")
    );
}

#[test]
fn parent_trigger_is_snapshotted_once_and_round_trips() {
    let Some(mut db) = TestSchema::new("pgpatch_t_parent_trg") else {
        return;
    };
    db.exec(&partitioned_table("pgpatch_t_parent_trg"));
    db.exec(
        "CREATE FUNCTION pgpatch_t_parent_trg.noop() RETURNS trigger LANGUAGE plpgsql \
         AS $$ BEGIN RETURN NEW; END $$;",
    );
    let without = db.snapshot();

    db.exec(
        "CREATE TRIGGER touch BEFORE INSERT ON pgpatch_t_parent_trg.job \
         FOR EACH ROW EXECUTE FUNCTION pgpatch_t_parent_trg.noop();",
    );
    let with = db.snapshot();
    let ns = &with.schemas["pgpatch_t_parent_trg"];
    assert!(ns.tables["job"].triggers.contains_key("touch"));
    assert!(
        ns.tables["job_common"].triggers.is_empty(),
        "cloned trigger must not be snapshotted on the partition: {:?}",
        ns.tables["job_common"].triggers
    );

    let to_without = diff::diff(&with, &without);
    assert!(!to_without.is_empty());
    db.apply(&to_without)
        .expect("dropping the parent trigger must succeed");
    assert_eq!(db.snapshot(), without);

    let to_with = diff::diff(&without, &with);
    assert!(!to_with.is_empty());
    db.apply(&to_with)
        .expect("creating the parent trigger must succeed");
    assert_eq!(db.snapshot(), with);
}

#[test]
fn plain_table_primary_key_round_trips() {
    let Some(mut db) = TestSchema::new("pgpatch_t_plain_pk") else {
        return;
    };
    db.exec("CREATE TABLE pgpatch_t_plain_pk.t (id int NOT NULL, v text);");
    let without = db.snapshot();

    db.exec("ALTER TABLE pgpatch_t_plain_pk.t ADD PRIMARY KEY (id);");
    let with = db.snapshot();

    // The primary key appears both as `primary_key` and as the `t_pkey`
    // constraint in the snapshot; the patch must add and drop it once.
    let to_without = diff::diff(&with, &without);
    assert!(!to_without.is_empty());
    db.apply(&to_without)
        .expect("dropping the pkey must succeed");
    assert_eq!(db.snapshot(), without);

    let to_with = diff::diff(&without, &with);
    assert!(!to_with.is_empty());
    db.apply(&to_with).expect("adding the pkey must succeed");
    assert_eq!(db.snapshot(), with);
}

#[test]
fn table_with_primary_key_is_created_from_scratch() {
    let Some(mut db) = TestSchema::new("pgpatch_t_create_pk") else {
        return;
    };
    let empty = db.snapshot();
    db.exec(
        "CREATE TABLE pgpatch_t_create_pk.t (id int NOT NULL, v text, \
         CONSTRAINT t_pk PRIMARY KEY (id), CONSTRAINT v_set CHECK (v IS NOT NULL));",
    );
    let with_table = db.snapshot();

    db.apply(&diff::diff(&with_table, &empty))
        .expect("dropping the table must succeed");
    assert_eq!(db.snapshot(), empty);

    let to_with = diff::diff(&empty, &with_table);
    assert!(!to_with.is_empty());
    db.apply(&to_with)
        .expect("creating a table with a primary key must succeed");
    assert_eq!(db.snapshot(), with_table);
}
