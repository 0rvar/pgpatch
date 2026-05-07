use pgpatch::diff::Change;
use pgpatch::emit::sql;
use pgpatch::model::{Column, QualifiedName, UserType};

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

fn qn(s: &str, n: &str) -> QualifiedName {
    QualifiedName::new(s, n)
}

fn position_of(haystack: &str, needle: &str) -> usize {
    haystack.find(needle).unwrap_or_else(|| panic!("missing {needle} in:\n{haystack}"))
}

#[test]
fn column_retyped_off_user_type_then_drop_type() {
    // Reference (target state) drops the enum entirely. The column that used
    // to be of that enum is now plain text. We must ALTER COLUMN before
    // DROP TYPE, otherwise PG refuses with "cannot drop type X because
    // other objects depend on it".
    let out = sql(&[
        Change::TypeRemoved { qual: qn("public", "status_enum") },
        Change::ColumnChanged {
            table: qn("public", "orders"),
            name: "status".into(),
            before: col("status", "public.status_enum", true),
            after: col("status", "text", true),
        },
    ]);

    let alter_col = position_of(&out, "ALTER TABLE public.orders ALTER COLUMN status TYPE text;");
    let drop_type = position_of(&out, "DROP TYPE public.status_enum;");
    assert!(
        alter_col < drop_type,
        "ALTER COLUMN must run before DROP TYPE; got:\n{out}"
    );
}

#[test]
fn create_type_then_column_retyped_to_new_type() {
    // Reverse direction: a new type is created and an existing column is
    // moved onto it. CREATE TYPE must come before ALTER COLUMN.
    let out = sql(&[
        Change::TypeAdded {
            qual: qn("public", "color"),
            user_type: UserType::Enum { values: vec!["red".into(), "green".into()] },
        },
        Change::ColumnChanged {
            table: qn("public", "shirts"),
            name: "shade".into(),
            before: col("shade", "text", true),
            after: col("shade", "public.color", true),
        },
    ]);

    let create_type = position_of(&out, "CREATE TYPE public.color AS ENUM");
    let alter_col = position_of(&out, "ALTER TABLE public.shirts ALTER COLUMN shade TYPE public.color;");
    assert!(create_type < alter_col, "CREATE TYPE must precede ALTER COLUMN; got:\n{out}");
}

#[test]
fn enum_value_added_before_column_uses_it() {
    // ALTER TYPE … ADD VALUE goes into other_changes. A subsequent
    // ColumnChanged that swaps the column's default to the new value would
    // fail if column_changes ran first. Verify the ordering: ADD VALUE
    // commits before column_changes apply.
    let out = sql(&[
        Change::TypeChanged {
            qual: qn("public", "color"),
            before: UserType::Enum { values: vec!["red".into()] },
            after: UserType::Enum { values: vec!["red".into(), "blue".into()] },
        },
        Change::ColumnChanged {
            table: qn("public", "shirts"),
            name: "shade".into(),
            before: col("shade", "public.color", true),
            after: Column {
                default: Some("'blue'::public.color".into()),
                ..col("shade", "public.color", true)
            },
        },
    ]);

    let add_value = position_of(&out, "ALTER TYPE public.color ADD VALUE IF NOT EXISTS 'blue';");
    let alter_default = position_of(&out, "ALTER TABLE public.shirts ALTER COLUMN shade SET DEFAULT 'blue'::public.color;");
    assert!(
        add_value < alter_default,
        "ADD VALUE must commit before column SET DEFAULT references it; got:\n{out}"
    );
}

#[test]
fn dropped_column_using_type_lets_type_drop_safely() {
    // Even with column_changes empty, dropping a column that referenced the
    // type happens in drop_columns (drop phase) — long before drop_types in
    // the new ordering, so DROP TYPE still succeeds.
    let out = sql(&[
        Change::ColumnRemoved { table: qn("public", "orders"), name: "status".into() },
        Change::TypeRemoved { qual: qn("public", "status_enum") },
    ]);
    let drop_col = position_of(&out, "ALTER TABLE public.orders DROP COLUMN status;");
    let drop_type = position_of(&out, "DROP TYPE public.status_enum;");
    assert!(drop_col < drop_type, "got:\n{out}");
}

#[test]
fn drop_type_still_runs_at_the_end_of_emission() {
    // drop_types is now the last drop. Document this contract via a snapshot
    // test: any future reordering needs to consciously break this.
    let out = sql(&[
        Change::TypeRemoved { qual: qn("public", "old_enum") },
        Change::ColumnChanged {
            table: qn("public", "t"),
            name: "c".into(),
            before: col("c", "public.old_enum", true),
            after: col("c", "text", true),
        },
    ]);
    // The DROP TYPE line should be on the *last* non-empty line.
    let last_line = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap();
    assert_eq!(last_line.trim(), "DROP TYPE public.old_enum;");
}
