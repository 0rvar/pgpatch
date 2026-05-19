// Tests for the SQL emitter, focused on the variants:
// SchemaAdded/Removed, ExtensionAdded/Removed/Changed,
// TriggerAdded/Removed/Changed, PolicyAdded/Removed/Changed,
// FunctionAdded/Removed/Changed, RlsEnabled/Disabled,
// plus bucket ordering across phases.

use pgpatch::diff::{Change, diff};
use pgpatch::emit::sql;
use pgpatch::model::{Extension, Function, Namespace, Policy, QualifiedName, Schema, Table, Trigger};

fn qn(schema: &str, name: &str) -> QualifiedName {
    QualifiedName::new(schema, name)
}

// --- SCHEMA ----------------------------------------------------------------

#[test]
fn schema_added_emits_create_if_not_exists() {
    let out = sql(&[Change::SchemaAdded { name: "analytics".into() }]);
    assert!(out.contains("CREATE SCHEMA IF NOT EXISTS analytics;"), "got: {out}");
}

#[test]
fn schema_added_quotes_mixed_case_identifier() {
    let out = sql(&[Change::SchemaAdded { name: "MySchema".into() }]);
    assert!(out.contains("CREATE SCHEMA IF NOT EXISTS \"MySchema\";"), "got: {out}");
}

#[test]
fn schema_removed_emits_drop_cascade() {
    let out = sql(&[Change::SchemaRemoved { name: "analytics".into() }]);
    assert!(out.contains("DROP SCHEMA analytics CASCADE;"), "got: {out}");
}

// --- EXTENSION -------------------------------------------------------------

#[test]
fn extension_added_emits_create_with_schema_and_quoted_version() {
    let out = sql(&[Change::ExtensionAdded {
        name: "pgcrypto".into(),
        extension: Extension { version: "1.3".into(), schema: "public".into() },
    }]);
    assert!(
        out.contains("CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public VERSION '1.3';"),
        "got: {out}",
    );
}

#[test]
fn extension_added_quotes_dashed_name() {
    // "uuid-ossp" needs identifier quoting because of the dash.
    let out = sql(&[Change::ExtensionAdded {
        name: "uuid-ossp".into(),
        extension: Extension { version: "1.1".into(), schema: "public".into() },
    }]);
    assert!(
        out.contains("CREATE EXTENSION IF NOT EXISTS \"uuid-ossp\" WITH SCHEMA public VERSION '1.1';"),
        "got: {out}",
    );
}

#[test]
fn extension_added_escapes_quote_in_version() {
    let out = sql(&[Change::ExtensionAdded {
        name: "weird".into(),
        extension: Extension { version: "1'2".into(), schema: "public".into() },
    }]);
    assert!(out.contains("VERSION '1''2';"), "got: {out}");
}

#[test]
fn extension_removed_emits_drop() {
    let out = sql(&[Change::ExtensionRemoved { name: "pgcrypto".into() }]);
    assert!(out.contains("DROP EXTENSION pgcrypto;"), "got: {out}");
}

#[test]
fn extension_changed_emits_alter_update_to() {
    let out = sql(&[Change::ExtensionChanged {
        name: "pgcrypto".into(),
        before: Extension { version: "1.2".into(), schema: "public".into() },
        after: Extension { version: "1.3".into(), schema: "public".into() },
    }]);
    assert!(out.contains("ALTER EXTENSION pgcrypto UPDATE TO '1.3';"), "got: {out}");
}

// --- TRIGGER ---------------------------------------------------------------

#[test]
fn trigger_added_emits_definition_terminated() {
    // Definition without trailing semicolon.
    let out = sql(&[Change::TriggerAdded {
        table: qn("public", "users"),
        name: "trg_audit".into(),
        trigger: Trigger {
            definition: "CREATE TRIGGER trg_audit BEFORE INSERT ON public.users FOR EACH ROW EXECUTE FUNCTION audit()".into(),
        },
    }]);
    assert!(out.contains("CREATE TRIGGER trg_audit"), "got: {out}");
    assert!(out.trim_end().ends_with(';'), "must end with ;: {out}");
    // Should not double-terminate.
    assert!(!out.contains(";;"), "double terminator: {out}");
}

#[test]
fn trigger_added_keeps_existing_terminator() {
    let out = sql(&[Change::TriggerAdded {
        table: qn("public", "users"),
        name: "trg_audit".into(),
        trigger: Trigger {
            definition: "CREATE TRIGGER trg_audit BEFORE INSERT ON public.users FOR EACH ROW EXECUTE FUNCTION audit();".into(),
        },
    }]);
    assert!(!out.contains(";;"), "must not double-terminate: {out}");
    assert!(out.contains("EXECUTE FUNCTION audit();"), "got: {out}");
}

#[test]
fn trigger_added_strips_trailing_whitespace_then_terminates() {
    // ensure_terminated trims trailing whitespace before checking ;.
    let out = sql(&[Change::TriggerAdded {
        table: qn("public", "users"),
        name: "trg_audit".into(),
        trigger: Trigger {
            definition: "CREATE TRIGGER trg_audit BEFORE INSERT ON public.users FOR EACH ROW EXECUTE FUNCTION audit();   \n".into(),
        },
    }]);
    assert!(!out.contains(";;"), "must not double-terminate: {out}");
}

#[test]
fn trigger_removed_emits_drop_on_table() {
    let out = sql(&[Change::TriggerRemoved {
        table: qn("public", "users"),
        name: "trg_audit".into(),
    }]);
    assert!(out.contains("DROP TRIGGER trg_audit ON public.users;"), "got: {out}");
}

#[test]
fn trigger_changed_emits_drop_then_create() {
    let out = sql(&[Change::TriggerChanged {
        table: qn("public", "users"),
        name: "trg_audit".into(),
        before: Trigger { definition: "CREATE TRIGGER trg_audit BEFORE INSERT ON public.users EXECUTE FUNCTION old()".into() },
        after: Trigger { definition: "CREATE TRIGGER trg_audit BEFORE INSERT ON public.users EXECUTE FUNCTION new()".into() },
    }]);
    let drop_idx = out.find("DROP TRIGGER trg_audit ON public.users;").expect("drop missing");
    let create_idx = out.find("EXECUTE FUNCTION new()").expect("create missing");
    assert!(drop_idx < create_idx, "drop must come before create: {out}");
}

// --- POLICY ----------------------------------------------------------------

#[test]
fn policy_added_permissive_for_all_no_for_clause() {
    let out = sql(&[Change::PolicyAdded {
        table: qn("public", "docs"),
        name: "p_all".into(),
        policy: Policy {
            command: "ALL".into(),
            permissive: true,
            roles: vec!["PUBLIC".into()],
            qual: None,
            with_check: None,
        },
    }]);
    assert!(out.contains("CREATE POLICY p_all ON public.docs"), "got: {out}");
    assert!(!out.contains(" AS RESTRICTIVE"), "permissive should omit RESTRICTIVE: {out}");
    assert!(!out.contains(" FOR "), "ALL command should omit FOR clause: {out}");
    assert!(out.contains(" TO PUBLIC"), "got: {out}");
}

#[test]
fn policy_added_restrictive_emits_as_restrictive() {
    let out = sql(&[Change::PolicyAdded {
        table: qn("public", "docs"),
        name: "p_r".into(),
        policy: Policy {
            command: "SELECT".into(),
            permissive: false,
            roles: vec![],
            qual: None,
            with_check: None,
        },
    }]);
    assert!(out.contains("CREATE POLICY p_r ON public.docs AS RESTRICTIVE FOR SELECT"), "got: {out}");
}

#[test]
fn policy_added_for_select_uppercases_command() {
    let out = sql(&[Change::PolicyAdded {
        table: qn("public", "docs"),
        name: "p_sel".into(),
        policy: Policy {
            command: "select".into(),
            permissive: true,
            roles: vec![],
            qual: None,
            with_check: None,
        },
    }]);
    assert!(out.contains(" FOR SELECT"), "got: {out}");
}

#[test]
fn policy_added_for_each_dml_command() {
    for cmd in ["INSERT", "UPDATE", "DELETE"] {
        let out = sql(&[Change::PolicyAdded {
            table: qn("public", "docs"),
            name: "p".into(),
            policy: Policy {
                command: cmd.into(),
                permissive: true,
                roles: vec![],
                qual: None,
                with_check: None,
            },
        }]);
        assert!(out.contains(&format!(" FOR {cmd}")), "missing FOR {cmd} in {out}");
    }
}

#[test]
fn policy_added_public_role_verbatim_uppercase() {
    // PUBLIC must not be identifier-quoted even though uppercase identifier
    // would otherwise be quoted.
    let out = sql(&[Change::PolicyAdded {
        table: qn("public", "docs"),
        name: "p".into(),
        policy: Policy {
            command: "SELECT".into(),
            permissive: true,
            roles: vec!["PUBLIC".into()],
            qual: None,
            with_check: None,
        },
    }]);
    assert!(out.contains(" TO PUBLIC"), "got: {out}");
    assert!(!out.contains("\"PUBLIC\""), "PUBLIC must not be quoted: {out}");
}

#[test]
fn policy_added_lowercase_role_unquoted() {
    // `authenticated` is a valid unquoted identifier — quote_ident leaves it.
    let out = sql(&[Change::PolicyAdded {
        table: qn("public", "docs"),
        name: "p".into(),
        policy: Policy {
            command: "SELECT".into(),
            permissive: true,
            roles: vec!["authenticated".into()],
            qual: None,
            with_check: None,
        },
    }]);
    assert!(out.contains(" TO authenticated"), "got: {out}");
    assert!(!out.contains("\"authenticated\""), "must not be quoted: {out}");
}

#[test]
fn policy_added_mixed_case_role_quoted() {
    // Mixed-case role identifiers DO get quoted.
    let out = sql(&[Change::PolicyAdded {
        table: qn("public", "docs"),
        name: "p".into(),
        policy: Policy {
            command: "SELECT".into(),
            permissive: true,
            roles: vec!["AppUser".into()],
            qual: None,
            with_check: None,
        },
    }]);
    assert!(out.contains(" TO \"AppUser\""), "got: {out}");
}

#[test]
fn policy_added_multiple_roles_comma_separated() {
    let out = sql(&[Change::PolicyAdded {
        table: qn("public", "docs"),
        name: "p".into(),
        policy: Policy {
            command: "SELECT".into(),
            permissive: true,
            roles: vec!["authenticated".into(), "PUBLIC".into()],
            qual: None,
            with_check: None,
        },
    }]);
    assert!(out.contains(" TO authenticated, PUBLIC"), "got: {out}");
}

#[test]
fn policy_added_with_qual() {
    let out = sql(&[Change::PolicyAdded {
        table: qn("public", "docs"),
        name: "p".into(),
        policy: Policy {
            command: "SELECT".into(),
            permissive: true,
            roles: vec![],
            qual: Some("user_id = current_user_id()".into()),
            with_check: None,
        },
    }]);
    assert!(out.contains(" USING (user_id = current_user_id())"), "got: {out}");
    assert!(!out.contains("WITH CHECK"), "got: {out}");
}

#[test]
fn policy_added_with_check_only() {
    let out = sql(&[Change::PolicyAdded {
        table: qn("public", "docs"),
        name: "p".into(),
        policy: Policy {
            command: "INSERT".into(),
            permissive: true,
            roles: vec![],
            qual: None,
            with_check: Some("owner = current_user".into()),
        },
    }]);
    assert!(out.contains(" WITH CHECK (owner = current_user)"), "got: {out}");
    assert!(!out.contains("USING"), "got: {out}");
}

#[test]
fn policy_added_full_clause_exact_snapshot() {
    let out = sql(&[Change::PolicyAdded {
        table: qn("public", "docs"),
        name: "p_full".into(),
        policy: Policy {
            command: "UPDATE".into(),
            permissive: false,
            roles: vec!["authenticated".into()],
            qual: Some("owner = current_user".into()),
            with_check: Some("owner = current_user".into()),
        },
    }]);
    assert_eq!(
        out,
        "CREATE POLICY p_full ON public.docs AS RESTRICTIVE FOR UPDATE TO authenticated USING (owner = current_user) WITH CHECK (owner = current_user);\n",
    );
}

#[test]
fn policy_removed_emits_drop() {
    let out = sql(&[Change::PolicyRemoved {
        table: qn("public", "docs"),
        name: "p".into(),
    }]);
    assert!(out.contains("DROP POLICY p ON public.docs;"), "got: {out}");
}

#[test]
fn policy_changed_emits_drop_then_create() {
    let before = Policy {
        command: "SELECT".into(),
        permissive: true,
        roles: vec!["PUBLIC".into()],
        qual: None,
        with_check: None,
    };
    let after = Policy {
        command: "SELECT".into(),
        permissive: true,
        roles: vec!["PUBLIC".into()],
        qual: Some("true".into()),
        with_check: None,
    };
    let out = sql(&[Change::PolicyChanged {
        table: qn("public", "docs"),
        name: "p".into(),
        before,
        after,
    }]);
    let drop_idx = out.find("DROP POLICY p ON public.docs;").expect("drop missing");
    let create_idx = out.find("CREATE POLICY p ON public.docs").expect("create missing");
    assert!(drop_idx < create_idx, "drop must come before create: {out}");
}

#[test]
fn policy_role_order_difference_is_not_a_change() {
    // Roles in a PG policy are a set — order is irrelevant for semantics.
    // Two snapshots that list the same roles in different orders must not
    // produce a PolicyChanged (otherwise we emit a spurious DROP+CREATE).
    let mut left_table = Table::default();
    left_table.policies.insert(
        "p".into(),
        Policy {
            command: "SELECT".into(),
            permissive: true,
            roles: vec!["anon".into(), "authenticated".into()],
            qual: None,
            with_check: None,
        },
    );
    let mut right_table = Table::default();
    right_table.policies.insert(
        "p".into(),
        Policy {
            command: "SELECT".into(),
            permissive: true,
            roles: vec!["authenticated".into(), "anon".into()],
            qual: None,
            with_check: None,
        },
    );

    let mut left = Schema::default();
    let mut right = Schema::default();
    let mut left_ns = Namespace::default();
    let mut right_ns = Namespace::default();
    left_ns.tables.insert("docs".into(), left_table);
    right_ns.tables.insert("docs".into(), right_table);
    left.schemas.insert("public".into(), left_ns);
    right.schemas.insert("public".into(), right_ns);

    let changes = diff(&left, &right);
    let has_policy_change = changes
        .iter()
        .any(|c| matches!(c, Change::PolicyChanged { .. }));
    assert!(
        !has_policy_change,
        "role-order-only difference must not be a PolicyChanged: {changes:#?}"
    );
}

// --- FUNCTION --------------------------------------------------------------

#[test]
fn function_added_emits_terminated_definition() {
    let out = sql(&[Change::FunctionAdded {
        qual: qn("public", "f(integer)"),
        function: Function {
            definition: "CREATE OR REPLACE FUNCTION public.f(x integer) RETURNS integer LANGUAGE sql AS $$ SELECT x $$".into(),
        },
    }]);
    assert!(out.contains("CREATE OR REPLACE FUNCTION public.f"), "got: {out}");
    assert!(out.trim_end().ends_with(';'), "got: {out}");
}

#[test]
fn function_removed_preserves_arg_signature_in_drop() {
    // qual.name carries the full function signature like `my_fn(integer, text)`.
    // The DROP must keep that verbatim — re-quoting would break the overload
    // resolution.
    let out = sql(&[Change::FunctionRemoved {
        qual: qn("public", "my_fn(integer, text)"),
    }]);
    assert!(
        out.contains("DROP FUNCTION public.my_fn(integer, text);"),
        "got: {out}",
    );
}

#[test]
fn function_changed_emits_create_or_replace_only() {
    let out = sql(&[Change::FunctionChanged {
        qual: qn("public", "f(integer)"),
        before: Function { definition: "CREATE OR REPLACE FUNCTION public.f(x integer) RETURNS integer LANGUAGE sql AS $$ SELECT 0 $$".into() },
        after: Function { definition: "CREATE OR REPLACE FUNCTION public.f(x integer) RETURNS integer LANGUAGE sql AS $$ SELECT x $$".into() },
    }]);
    assert!(out.contains("SELECT x"), "got: {out}");
    assert!(!out.contains("DROP FUNCTION"), "should not drop on in-place change: {out}");
    assert!(!out.contains("SELECT 0"), "must not include before-definition: {out}");
}

// --- RLS -------------------------------------------------------------------

#[test]
fn rls_enabled_emits_alter_table_enable() {
    let out = sql(&[Change::RlsEnabled { table: qn("public", "docs") }]);
    assert!(out.contains("ALTER TABLE public.docs ENABLE ROW LEVEL SECURITY;"), "got: {out}");
}

#[test]
fn rls_disabled_emits_alter_table_disable() {
    let out = sql(&[Change::RlsDisabled { table: qn("public", "docs") }]);
    assert!(out.contains("ALTER TABLE public.docs DISABLE ROW LEVEL SECURITY;"), "got: {out}");
}

// --- BUCKET ORDERING -------------------------------------------------------

// Drops must precede creates across phases. Within drops, policies and
// triggers come before columns and tables. Within creates, extensions and
// schemas come before tables, triggers and policies.
#[test]
fn drop_policies_before_drop_table() {
    let out = sql(&[
        Change::TableRemoved { qual: qn("public", "docs") },
        Change::PolicyRemoved { table: qn("public", "docs"), name: "p".into() },
    ]);
    let policy_idx = out.find("DROP POLICY p ON public.docs;").expect("drop policy missing");
    let table_idx = out.find("DROP TABLE public.docs;").expect("drop table missing");
    assert!(policy_idx < table_idx, "drop policy must precede drop table: {out}");
}

#[test]
fn drop_triggers_before_drop_columns_and_tables() {
    let out = sql(&[
        Change::TableRemoved { qual: qn("public", "x") },
        Change::ColumnRemoved { table: qn("public", "x"), name: "c".into() },
        Change::TriggerRemoved { table: qn("public", "x"), name: "trg".into() },
    ]);
    let trg = out.find("DROP TRIGGER").unwrap();
    let col = out.find("DROP COLUMN").unwrap();
    let tbl = out.find("DROP TABLE").unwrap();
    assert!(trg < col, "trigger drop before column drop: {out}");
    assert!(col < tbl, "column drop before table drop: {out}");
}

#[test]
fn create_extensions_before_create_schemas_before_create_triggers_and_policies() {
    let out = sql(&[
        Change::TriggerAdded {
            table: qn("public", "t"),
            name: "trg".into(),
            trigger: Trigger { definition: "CREATE TRIGGER trg BEFORE INSERT ON public.t FOR EACH ROW EXECUTE FUNCTION f()".into() },
        },
        Change::PolicyAdded {
            table: qn("public", "t"),
            name: "p".into(),
            policy: Policy {
                command: "SELECT".into(),
                permissive: true,
                roles: vec![],
                qual: None,
                with_check: None,
            },
        },
        Change::SchemaAdded { name: "analytics".into() },
        Change::ExtensionAdded {
            name: "pgcrypto".into(),
            extension: Extension { version: "1.3".into(), schema: "public".into() },
        },
    ]);
    let ext = out.find("CREATE EXTENSION").unwrap();
    let sch = out.find("CREATE SCHEMA").unwrap();
    let trg = out.find("CREATE TRIGGER").unwrap();
    let pol = out.find("CREATE POLICY").unwrap();
    assert!(ext < sch, "extensions before schemas: {out}");
    assert!(sch < trg, "schemas before triggers: {out}");
    assert!(trg < pol, "triggers before policies: {out}");
}

#[test]
fn drops_before_creates_across_all_buckets() {
    let out = sql(&[
        Change::SchemaAdded { name: "new_s".into() },
        Change::PolicyRemoved { table: qn("public", "docs"), name: "old_p".into() },
        Change::ExtensionAdded {
            name: "pgcrypto".into(),
            extension: Extension { version: "1.3".into(), schema: "public".into() },
        },
        Change::TriggerRemoved { table: qn("public", "docs"), name: "old_trg".into() },
    ]);
    let drop_pol = out.find("DROP POLICY old_p").unwrap();
    let drop_trg = out.find("DROP TRIGGER old_trg").unwrap();
    let create_ext = out.find("CREATE EXTENSION").unwrap();
    let create_sch = out.find("CREATE SCHEMA").unwrap();
    assert!(drop_pol < create_ext, "drops before creates: {out}");
    assert!(drop_trg < create_sch, "drops before creates: {out}");
}

#[test]
fn rls_alter_emitted_after_creates() {
    // RLS toggles live in the trailing alter-bucket, after creates.
    let out = sql(&[
        Change::RlsEnabled { table: qn("public", "docs") },
        Change::SchemaAdded { name: "s".into() },
    ]);
    let create = out.find("CREATE SCHEMA").unwrap();
    let rls = out.find("ENABLE ROW LEVEL SECURITY").unwrap();
    assert!(create < rls, "creates before RLS toggle: {out}");
}

#[test]
fn extension_change_emitted_in_alter_bucket_after_creates() {
    let out = sql(&[
        Change::SchemaAdded { name: "s".into() },
        Change::ExtensionChanged {
            name: "pgcrypto".into(),
            before: Extension { version: "1.2".into(), schema: "public".into() },
            after: Extension { version: "1.3".into(), schema: "public".into() },
        },
    ]);
    let create = out.find("CREATE SCHEMA").unwrap();
    let alter = out.find("ALTER EXTENSION").unwrap();
    assert!(create < alter, "create before alter extension: {out}");
}

#[test]
fn function_changed_emitted_in_other_changes_bucket() {
    // FunctionChanged goes into the trailing other_changes bucket, so it
    // emits after CREATE TABLE and friends.
    let out = sql(&[
        Change::FunctionChanged {
            qual: qn("public", "f(integer)"),
            before: Function { definition: "CREATE OR REPLACE FUNCTION public.f(x integer) RETURNS integer LANGUAGE sql AS $$ SELECT 0 $$".into() },
            after: Function { definition: "CREATE OR REPLACE FUNCTION public.f(x integer) RETURNS integer LANGUAGE sql AS $$ SELECT x $$".into() },
        },
        Change::SchemaAdded { name: "s".into() },
    ]);
    let create = out.find("CREATE SCHEMA").unwrap();
    let func = out.find("SELECT x").unwrap();
    assert!(create < func, "schema create before function in-place change: {out}");
}

// --- MULTI-CHANGE EXACT SNAPSHOT ------------------------------------------

#[test]
fn multi_change_exact_snapshot() {
    // A composite scenario verifying ordering and exact text of every line.
    let out = sql(&[
        Change::SchemaAdded { name: "analytics".into() },
        Change::ExtensionAdded {
            name: "pgcrypto".into(),
            extension: Extension { version: "1.3".into(), schema: "public".into() },
        },
        Change::PolicyRemoved { table: qn("public", "docs"), name: "old_p".into() },
        Change::TriggerRemoved { table: qn("public", "docs"), name: "old_trg".into() },
        Change::RlsEnabled { table: qn("public", "docs") },
    ]);
    // The emitter separates statements with a blank line for readability.
    let expected = "\
DROP POLICY old_p ON public.docs;

DROP TRIGGER old_trg ON public.docs;

CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public VERSION '1.3';

CREATE SCHEMA IF NOT EXISTS analytics;

ALTER TABLE public.docs ENABLE ROW LEVEL SECURITY;
";
    assert_eq!(out, expected, "actual:\n{out}");
}
