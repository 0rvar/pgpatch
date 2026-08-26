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
