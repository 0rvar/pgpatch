// Tests for the SQL emitter's coverage of VIEW, MATERIALIZED VIEW, SEQUENCE,
// and USER TYPE change variants.

use pgpatch::diff::Change;
use pgpatch::emit::sql;
use pgpatch::model::{QualifiedName, Sequence, UserType, View};

fn qn(schema: &str, name: &str) -> QualifiedName {
    QualifiedName::new(schema, name)
}

fn seq_default() -> Sequence {
    Sequence {
        data_type: "bigint".into(),
        start: 1,
        increment: 1,
        min_value: 1,
        max_value: 9223372036854775807,
        cache: 1,
        cycle: false,
        owned_by: None,
    }
}

// ---------- Views ----------

#[test]
fn view_added_non_materialized() {
    let view = View {
        definition: "SELECT 1 AS x".into(),
        ..Default::default()
    };
    let out = sql(&[Change::ViewAdded {
        qual: qn("public", "v1"),
        materialized: false,
        view,
    }]);
    assert!(out.contains("CREATE VIEW public.v1 AS"));
    assert!(out.contains("SELECT 1 AS x;"));
    assert!(!out.contains("MATERIALIZED"));
}

#[test]
fn view_added_materialized() {
    let view = View {
        definition: "SELECT 2 AS y".into(),
        ..Default::default()
    };
    let out = sql(&[Change::ViewAdded {
        qual: qn("public", "mv"),
        materialized: true,
        view,
    }]);
    assert!(out.contains("CREATE MATERIALIZED VIEW public.mv AS"));
    assert!(out.contains("SELECT 2 AS y;"));
}

#[test]
fn view_added_definition_already_terminated() {
    // ensure_terminated should not double the semicolon.
    let view = View {
        definition: "SELECT 3;".into(),
        ..Default::default()
    };
    let out = sql(&[Change::ViewAdded {
        qual: qn("public", "v"),
        materialized: false,
        view,
    }]);
    assert!(out.contains("SELECT 3;"));
    assert!(!out.contains("SELECT 3;;"));
}

#[test]
fn view_removed_both_kinds() {
    let out = sql(&[Change::ViewRemoved {
        qual: qn("public", "v1"),
        materialized: false,
        depends_on: vec![],
    }]);
    assert_eq!(out, "DROP VIEW public.v1;\n");

    let out = sql(&[Change::ViewRemoved {
        qual: qn("public", "mv1"),
        materialized: true,
        depends_on: vec![],
    }]);
    assert_eq!(out, "DROP MATERIALIZED VIEW public.mv1;\n");
}

#[test]
fn view_changed_emits_drop_then_create() {
    let before = View {
        definition: "SELECT 1".into(),
        ..Default::default()
    };
    let after = View {
        definition: "SELECT 2".into(),
        ..Default::default()
    };
    let out = sql(&[Change::ViewChanged {
        qual: qn("public", "v"),
        materialized: false,
        before,
        after,
    }]);
    let drop_pos = out.find("DROP VIEW public.v;").expect("drop missing");
    let create_pos = out.find("CREATE VIEW public.v AS").expect("create missing");
    assert!(drop_pos < create_pos, "drop must precede create:\n{out}");
    assert!(out.contains("SELECT 2;"));
}

#[test]
fn view_changed_materialized() {
    let before = View {
        definition: "SELECT 1".into(),
        ..Default::default()
    };
    let after = View {
        definition: "SELECT 2".into(),
        ..Default::default()
    };
    let out = sql(&[Change::ViewChanged {
        qual: qn("public", "mv"),
        materialized: true,
        before,
        after,
    }]);
    assert!(out.contains("DROP MATERIALIZED VIEW public.mv;"));
    assert!(out.contains("CREATE MATERIALIZED VIEW public.mv AS"));
}

// ---------- Sequences ----------

#[test]
fn sequence_added_full_emit_no_owner() {
    let s = Sequence {
        data_type: "integer".into(),
        start: 5,
        increment: 2,
        min_value: 1,
        max_value: 1000,
        cache: 10,
        cycle: false,
        owned_by: None,
    };
    let out = sql(&[Change::SequenceAdded {
        qual: qn("public", "s1"),
        sequence: s,
    }]);
    let expected = "CREATE SEQUENCE public.s1 AS integer INCREMENT BY 2 MINVALUE 1 MAXVALUE 1000 START WITH 5 CACHE 10 NO CYCLE;\n";
    assert_eq!(out, expected);
}

#[test]
fn sequence_added_with_cycle_and_owner() {
    let s = Sequence {
        data_type: "bigint".into(),
        start: 1,
        increment: 1,
        min_value: 1,
        max_value: 9999,
        cache: 1,
        cycle: true,
        owned_by: Some("public.t.id".into()),
    };
    let out = sql(&[Change::SequenceAdded {
        qual: qn("public", "s2"),
        sequence: s,
    }]);
    assert!(out.contains(" CYCLE"));
    assert!(!out.contains("NO CYCLE"));
    assert!(out.contains("OWNED BY public.t.id"));
}

#[test]
fn sequence_removed() {
    let out = sql(&[Change::SequenceRemoved {
        qual: qn("public", "s1"),
    }]);
    assert_eq!(out, "DROP SEQUENCE public.s1;\n");
}

#[test]
fn sequence_changed_data_type_only() {
    let before = seq_default();
    let mut after = seq_default();
    after.data_type = "integer".into();
    let out = sql(&[Change::SequenceChanged {
        qual: qn("public", "s"),
        before,
        after,
    }]);
    assert_eq!(out, "ALTER SEQUENCE public.s AS integer;\n");
}

#[test]
fn sequence_changed_increment_only() {
    let before = seq_default();
    let mut after = seq_default();
    after.increment = 5;
    let out = sql(&[Change::SequenceChanged {
        qual: qn("public", "s"),
        before,
        after,
    }]);
    assert_eq!(out, "ALTER SEQUENCE public.s INCREMENT BY 5;\n");
}

#[test]
fn sequence_changed_min_max_only() {
    let before = seq_default();
    let mut after = seq_default();
    after.min_value = 10;
    after.max_value = 100;
    let out = sql(&[Change::SequenceChanged {
        qual: qn("public", "s"),
        before,
        after,
    }]);
    assert_eq!(out, "ALTER SEQUENCE public.s MINVALUE 10 MAXVALUE 100;\n");
}

#[test]
fn sequence_changed_start_and_cache() {
    let before = seq_default();
    let mut after = seq_default();
    after.start = 42;
    after.cache = 7;
    let out = sql(&[Change::SequenceChanged {
        qual: qn("public", "s"),
        before,
        after,
    }]);
    assert_eq!(out, "ALTER SEQUENCE public.s START WITH 42 CACHE 7;\n");
}

#[test]
fn sequence_changed_cycle_toggle() {
    let before = seq_default();
    let mut after = seq_default();
    after.cycle = true;
    let out = sql(&[Change::SequenceChanged {
        qual: qn("public", "s"),
        before,
        after,
    }]);
    assert_eq!(out, "ALTER SEQUENCE public.s CYCLE;\n");

    let mut before = seq_default();
    before.cycle = true;
    let after = seq_default();
    let out = sql(&[Change::SequenceChanged {
        qual: qn("public", "s"),
        before,
        after,
    }]);
    assert_eq!(out, "ALTER SEQUENCE public.s NO CYCLE;\n");
}

#[test]
fn sequence_changed_owned_by_set_and_clear() {
    let before = seq_default();
    let mut after = seq_default();
    after.owned_by = Some("public.t.id".into());
    let out = sql(&[Change::SequenceChanged {
        qual: qn("public", "s"),
        before,
        after,
    }]);
    assert_eq!(out, "ALTER SEQUENCE public.s OWNED BY public.t.id;\n");

    let mut before = seq_default();
    before.owned_by = Some("public.t.id".into());
    let after = seq_default();
    let out = sql(&[Change::SequenceChanged {
        qual: qn("public", "s"),
        before,
        after,
    }]);
    assert_eq!(out, "ALTER SEQUENCE public.s OWNED BY NONE;\n");
}

#[test]
fn sequence_changed_multi_axis_snapshot() {
    let before = seq_default();
    let after = Sequence {
        data_type: "integer".into(),
        start: 100,
        increment: 2,
        min_value: 50,
        max_value: 500,
        cache: 20,
        cycle: true,
        owned_by: Some("public.t.col".into()),
    };
    let out = sql(&[Change::SequenceChanged {
        qual: qn("public", "s"),
        before,
        after,
    }]);
    let expected = "ALTER SEQUENCE public.s AS integer INCREMENT BY 2 MINVALUE 50 MAXVALUE 500 START WITH 100 CACHE 20 CYCLE OWNED BY public.t.col;\n";
    assert_eq!(out, expected);
}

#[test]
fn sequence_changed_noop_emits_comment() {
    // Constructing SequenceChanged with identical before/after should fall
    // through to the no-op branch — diff would never produce this, but the
    // emitter must remain robust.
    let s = seq_default();
    let out = sql(&[Change::SequenceChanged {
        qual: qn("public", "s"),
        before: s.clone(),
        after: s,
    }]);
    assert_eq!(out, "-- no-op sequence change on public.s\n");
}

// ---------- User types ----------

#[test]
fn type_added_enum_quotes_values() {
    let t = UserType::Enum {
        values: vec!["red".into(), "blue".into(), "it's green".into()],
    };
    let out = sql(&[Change::TypeAdded {
        qual: qn("public", "color"),
        user_type: t,
    }]);
    assert_eq!(
        out,
        "CREATE TYPE public.color AS ENUM ('red', 'blue', 'it''s green');\n",
    );
}

#[test]
fn type_added_composite() {
    let t = UserType::Composite {
        fields: vec![
            ("a".into(), "integer".into()),
            ("b".into(), "text".into()),
        ],
    };
    let out = sql(&[Change::TypeAdded {
        qual: qn("public", "pair"),
        user_type: t,
    }]);
    let expected = "CREATE TYPE public.pair AS (\n    a integer,\n    b text\n);\n";
    assert_eq!(out, expected);
}

#[test]
fn type_added_domain_with_and_without_definition() {
    let t = UserType::Domain {
        base_type: "integer".into(),
        definition: String::new(),
    };
    let out = sql(&[Change::TypeAdded {
        qual: qn("public", "pos"),
        user_type: t,
    }]);
    assert_eq!(out, "CREATE DOMAIN public.pos AS integer;\n");

    let t = UserType::Domain {
        base_type: "integer".into(),
        definition: "CHECK (VALUE > 0)".into(),
    };
    let out = sql(&[Change::TypeAdded {
        qual: qn("public", "pos"),
        user_type: t,
    }]);
    assert_eq!(out, "CREATE DOMAIN public.pos AS integer CHECK (VALUE > 0);\n");
}

#[test]
fn type_added_range_with_and_without_definition() {
    let t = UserType::Range {
        subtype: "integer".into(),
        definition: String::new(),
    };
    let out = sql(&[Change::TypeAdded {
        qual: qn("public", "intr"),
        user_type: t,
    }]);
    assert_eq!(out, "CREATE TYPE public.intr AS RANGE (SUBTYPE = integer);\n");

    let t = UserType::Range {
        subtype: "integer".into(),
        definition: "SUBTYPE = integer, SUBTYPE_OPCLASS = int4_ops".into(),
    };
    let out = sql(&[Change::TypeAdded {
        qual: qn("public", "intr"),
        user_type: t,
    }]);
    assert_eq!(
        out,
        "CREATE TYPE public.intr AS RANGE (SUBTYPE = integer, SUBTYPE_OPCLASS = int4_ops);\n",
    );
}

#[test]
fn type_removed() {
    let out = sql(&[Change::TypeRemoved {
        qual: qn("public", "color"),
    }]);
    assert_eq!(out, "DROP TYPE public.color;\n");
}

#[test]
fn type_changed_enum_added_value() {
    let before = UserType::Enum {
        values: vec!["red".into(), "blue".into()],
    };
    let after = UserType::Enum {
        values: vec!["red".into(), "blue".into(), "green".into()],
    };
    let out = sql(&[Change::TypeChanged {
        qual: qn("public", "color"),
        before,
        after,
    }]);
    assert_eq!(
        out,
        "ALTER TYPE public.color ADD VALUE IF NOT EXISTS 'green';\n",
    );
}

#[test]
fn type_changed_enum_added_value_with_apostrophe() {
    let before = UserType::Enum {
        values: vec!["a".into()],
    };
    let after = UserType::Enum {
        values: vec!["a".into(), "it's b".into()],
    };
    let out = sql(&[Change::TypeChanged {
        qual: qn("public", "t"),
        before,
        after,
    }]);
    assert!(out.contains("ADD VALUE IF NOT EXISTS 'it''s b'"));
}

#[test]
fn type_changed_enum_removed_value_emits_todo() {
    let before = UserType::Enum {
        values: vec!["a".into(), "b".into()],
    };
    let after = UserType::Enum {
        values: vec!["a".into()],
    };
    let out = sql(&[Change::TypeChanged {
        qual: qn("public", "t"),
        before,
        after,
    }]);
    assert!(out.contains("-- TODO"));
    assert!(out.contains("'b'"));
    assert!(out.contains("PG cannot drop enum values"));
    assert!(!out.contains("ALTER TYPE"));
}

#[test]
fn type_changed_enum_reorder_only_emits_comment() {
    let before = UserType::Enum {
        values: vec!["a".into(), "b".into()],
    };
    let after = UserType::Enum {
        values: vec!["b".into(), "a".into()],
    };
    let out = sql(&[Change::TypeChanged {
        qual: qn("public", "t"),
        before,
        after,
    }]);
    assert_eq!(out, "-- enum public.t reordered (no SQL needed)\n");
}

#[test]
fn type_changed_non_enum_emits_todo() {
    // Composite -> Composite with different fields.
    let before = UserType::Composite {
        fields: vec![("a".into(), "integer".into())],
    };
    let after = UserType::Composite {
        fields: vec![("a".into(), "bigint".into())],
    };
    let out = sql(&[Change::TypeChanged {
        qual: qn("public", "c"),
        before,
        after,
    }]);
    assert_eq!(
        out,
        "-- TODO: type change on public.c requires manual migration\n",
    );

    // Domain change.
    let before = UserType::Domain {
        base_type: "integer".into(),
        definition: String::new(),
    };
    let after = UserType::Domain {
        base_type: "bigint".into(),
        definition: String::new(),
    };
    let out = sql(&[Change::TypeChanged {
        qual: qn("public", "d"),
        before,
        after,
    }]);
    assert!(out.contains("-- TODO: type change on public.d"));

    // Range change.
    let before = UserType::Range {
        subtype: "integer".into(),
        definition: String::new(),
    };
    let after = UserType::Range {
        subtype: "bigint".into(),
        definition: String::new(),
    };
    let out = sql(&[Change::TypeChanged {
        qual: qn("public", "r"),
        before,
        after,
    }]);
    assert!(out.contains("-- TODO: type change on public.r"));

    // Cross-kind (enum -> composite) is also non-enum-on-both-sides.
    let before = UserType::Enum {
        values: vec!["a".into()],
    };
    let after = UserType::Composite {
        fields: vec![("a".into(), "integer".into())],
    };
    let out = sql(&[Change::TypeChanged {
        qual: qn("public", "x"),
        before,
        after,
    }]);
    assert!(out.contains("-- TODO: type change on public.x"));
}
