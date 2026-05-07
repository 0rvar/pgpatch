// Tests for the emit module covering the index and constraint Change variants.
// Other Change variants (tables/columns/PK, views/sequences/types,
// triggers/policies/functions/RLS/schemas/extensions) are covered by sibling
// integration tests.

use pgpatch::diff::Change;
use pgpatch::emit;
use pgpatch::model::{Constraint, Index, QualifiedName};

fn qual(schema: &str, name: &str) -> QualifiedName {
    QualifiedName::new(schema, name)
}

fn idx(definition: &str) -> Index {
    Index { definition: definition.to_string(), unique: false, primary: false }
}

fn unique_idx(definition: &str) -> Index {
    Index { definition: definition.to_string(), unique: true, primary: false }
}

fn ck(definition: &str) -> Constraint {
    Constraint { kind: "c".into(), definition: definition.to_string() }
}

fn fk(definition: &str) -> Constraint {
    Constraint { kind: "f".into(), definition: definition.to_string() }
}

// ---------- IndexAdded ----------

#[test]
fn index_added_emits_definition_verbatim_with_terminator() {
    let change = Change::IndexAdded {
        table: qual("public", "users"),
        name: "users_email_idx".into(),
        index: idx("CREATE INDEX users_email_idx ON public.users USING btree (email)"),
    };
    let out = emit::sql(&[change]);
    assert_eq!(
        out.trim(),
        "CREATE INDEX users_email_idx ON public.users USING btree (email);"
    );
}

#[test]
fn index_added_unique_definition_passed_through() {
    // The emitter is supposed to hand the pg_get_indexdef string back as-is;
    // it should not interpret or rewrite the UNIQUE keyword.
    let change = Change::IndexAdded {
        table: qual("public", "t"),
        name: "t_x_uniq".into(),
        index: unique_idx("CREATE UNIQUE INDEX t_x_uniq ON public.t USING btree (x)"),
    };
    let out = emit::sql(&[change]);
    assert!(out.contains("CREATE UNIQUE INDEX t_x_uniq ON public.t USING btree (x);"));
}

// ---------- IndexRemoved ----------

#[test]
fn index_removed_emits_schema_qualified_drop() {
    let change = Change::IndexRemoved {
        table: qual("public", "users"),
        name: "users_email_idx".into(),
    };
    let out = emit::sql(&[change]);
    assert_eq!(out.trim(), "DROP INDEX public.users_email_idx;");
}

#[test]
fn index_removed_quotes_reserved_schema_and_mixed_case_name() {
    // "user" is in the reserved-keyword list and "MyIndex" has uppercase
    // letters; both must be double-quoted.
    let change = Change::IndexRemoved {
        table: qual("user", "t"),
        name: "MyIndex".into(),
    };
    let out = emit::sql(&[change]);
    assert_eq!(out.trim(), "DROP INDEX \"user\".\"MyIndex\";");
}

// ---------- IndexChanged ----------

#[test]
fn index_changed_emits_drop_then_create() {
    let change = Change::IndexChanged {
        table: qual("public", "users"),
        name: "users_email_idx".into(),
        before: idx("CREATE INDEX users_email_idx ON public.users (email)"),
        after: idx("CREATE INDEX users_email_idx ON public.users (lower(email))"),
    };
    let out = emit::sql(&[change]);
    assert!(out.contains("DROP INDEX public.users_email_idx;"));
    assert!(out.contains("CREATE INDEX users_email_idx ON public.users (lower(email));"));
    let drop_pos = out.find("DROP INDEX").unwrap();
    let create_pos = out.find("CREATE INDEX").unwrap();
    assert!(drop_pos < create_pos, "drop must precede create");
}

// ---------- ConstraintAdded ----------

#[test]
fn constraint_added_emits_alter_table_add_constraint() {
    let change = Change::ConstraintAdded {
        table: qual("public", "users"),
        name: "users_age_check".into(),
        constraint: ck("CHECK (age >= 0)"),
    };
    let out = emit::sql(&[change]);
    assert_eq!(
        out.trim(),
        "ALTER TABLE public.users ADD CONSTRAINT users_age_check CHECK (age >= 0);"
    );
}

#[test]
fn constraint_added_quotes_capitalised_name_and_reserved_schema() {
    let change = Change::ConstraintAdded {
        table: qual("user", "orders"),
        name: "FK_Orders_User".into(),
        constraint: fk("FOREIGN KEY (user_id) REFERENCES \"user\".accounts(id)"),
    };
    let out = emit::sql(&[change]);
    assert!(out.contains("ALTER TABLE \"user\".orders ADD CONSTRAINT \"FK_Orders_User\""));
    assert!(out.contains("FOREIGN KEY (user_id) REFERENCES \"user\".accounts(id);"));
}

// ---------- ConstraintRemoved ----------

#[test]
fn constraint_removed_emits_alter_table_drop_constraint() {
    let change = Change::ConstraintRemoved {
        table: qual("public", "users"),
        name: "users_age_check".into(),
    };
    let out = emit::sql(&[change]);
    assert_eq!(
        out.trim(),
        "ALTER TABLE public.users DROP CONSTRAINT users_age_check;"
    );
}

#[test]
fn constraint_removed_quotes_capitalised_name() {
    let change = Change::ConstraintRemoved {
        table: qual("public", "orders"),
        name: "FK_Orders_User".into(),
    };
    let out = emit::sql(&[change]);
    assert_eq!(
        out.trim(),
        "ALTER TABLE public.orders DROP CONSTRAINT \"FK_Orders_User\";"
    );
}

// ---------- ConstraintChanged ----------

#[test]
fn constraint_changed_emits_drop_then_add_in_order() {
    let change = Change::ConstraintChanged {
        table: qual("public", "users"),
        name: "users_age_check".into(),
        before: ck("CHECK (age >= 0)"),
        after: ck("CHECK (age >= 18)"),
    };
    let out = emit::sql(&[change]);
    assert!(out.contains("ALTER TABLE public.users DROP CONSTRAINT users_age_check;"));
    assert!(out.contains(
        "ALTER TABLE public.users ADD CONSTRAINT users_age_check CHECK (age >= 18);"
    ));
    let drop_pos = out.find("DROP CONSTRAINT").unwrap();
    let add_pos = out.find("ADD CONSTRAINT").unwrap();
    assert!(drop_pos < add_pos, "drop must precede add");
    // The new (after) definition is emitted; the old must not appear.
    assert!(!out.contains("age >= 0"));
}

// ---------- Bucket ordering ----------

#[test]
fn drops_precede_creates_across_indexes_and_constraints() {
    // Mix every variant in the slice; the emitter must group drops first
    // (constraint drops before index drops) and creates after (constraints
    // before indexes), per Buckets::into_ordered.
    let changes = vec![
        Change::IndexAdded {
            table: qual("public", "users"),
            name: "users_email_idx".into(),
            index: idx("CREATE INDEX users_email_idx ON public.users (email)"),
        },
        Change::ConstraintAdded {
            table: qual("public", "users"),
            name: "users_age_chk".into(),
            constraint: ck("CHECK (age >= 0)"),
        },
        Change::IndexRemoved {
            table: qual("public", "users"),
            name: "old_idx".into(),
        },
        Change::ConstraintRemoved {
            table: qual("public", "users"),
            name: "old_chk".into(),
        },
    ];
    let out = emit::sql(&changes);

    let p_drop_con = out.find("DROP CONSTRAINT old_chk").unwrap();
    let p_drop_idx = out.find("DROP INDEX public.old_idx").unwrap();
    let p_add_con = out.find("ADD CONSTRAINT users_age_chk").unwrap();
    let p_add_idx = out.find("CREATE INDEX users_email_idx").unwrap();

    assert!(p_drop_con < p_drop_idx, "constraint drops before index drops");
    assert!(p_drop_idx < p_add_con, "all drops before all creates");
    assert!(p_add_con < p_add_idx, "constraint adds before index adds");
}

#[test]
fn changed_index_drop_precedes_added_constraint_create() {
    // IndexChanged contributes both a drop and a create. Across a mixed
    // bucket, the drop half must come out before any create half (including
    // an unrelated ConstraintAdded).
    let changes = vec![
        Change::ConstraintAdded {
            table: qual("public", "t"),
            name: "t_chk".into(),
            constraint: ck("CHECK (x > 0)"),
        },
        Change::IndexChanged {
            table: qual("public", "t"),
            name: "t_idx".into(),
            before: idx("CREATE INDEX t_idx ON public.t (x)"),
            after: idx("CREATE INDEX t_idx ON public.t (x, y)"),
        },
    ];
    let out = emit::sql(&changes);

    let drop_pos = out.find("DROP INDEX public.t_idx").unwrap();
    let add_con_pos = out.find("ADD CONSTRAINT t_chk").unwrap();
    let create_idx_pos = out.find("CREATE INDEX t_idx ON public.t (x, y)").unwrap();
    assert!(drop_pos < add_con_pos);
    assert!(add_con_pos < create_idx_pos);
}

#[test]
fn input_order_preserved_within_a_bucket() {
    // Two ConstraintAdded changes land in the same bucket; emit::sql must
    // keep them in input order (diff already sorts upstream).
    let changes = vec![
        Change::ConstraintAdded {
            table: qual("public", "t"),
            name: "z_last".into(),
            constraint: ck("CHECK (z > 0)"),
        },
        Change::ConstraintAdded {
            table: qual("public", "t"),
            name: "a_first".into(),
            constraint: ck("CHECK (a > 0)"),
        },
    ];
    let out = emit::sql(&changes);
    let z_pos = out.find("z_last").unwrap();
    let a_pos = out.find("a_first").unwrap();
    assert!(z_pos < a_pos, "input order preserved within create_constraints bucket");
}

// ---------- Exact-string snapshot ----------

#[test]
fn multi_change_snapshot_is_exact() {
    let changes = vec![
        Change::IndexRemoved {
            table: qual("public", "users"),
            name: "users_old_idx".into(),
        },
        Change::ConstraintRemoved {
            table: qual("public", "users"),
            name: "users_old_chk".into(),
        },
        Change::IndexAdded {
            table: qual("public", "users"),
            name: "users_new_idx".into(),
            index: idx("CREATE INDEX users_new_idx ON public.users (email)"),
        },
        Change::ConstraintAdded {
            table: qual("public", "users"),
            name: "users_new_chk".into(),
            constraint: ck("CHECK (age >= 18)"),
        },
    ];
    let out = emit::sql(&changes);
    let expected = "\
ALTER TABLE public.users DROP CONSTRAINT users_old_chk;

DROP INDEX public.users_old_idx;

ALTER TABLE public.users ADD CONSTRAINT users_new_chk CHECK (age >= 18);

CREATE INDEX users_new_idx ON public.users (email);
";
    assert_eq!(out, expected);
}
