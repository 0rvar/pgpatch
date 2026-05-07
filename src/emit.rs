use crate::diff::Change;
use crate::model::{Column, Identity, QualifiedName, Sequence, Table, UserType};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// One pending view DROP/CREATE statement, paired with the name and
/// dependency list needed to topo-sort the bucket before flushing.
#[derive(Debug, Clone)]
struct ViewSlot {
    qual: String,
    depends_on: Vec<String>,
    sql: String,
}

// Best-effort SQL emission. For each Change we produce DDL that, when applied
// to the right-hand side of a diff, brings it to the left-hand side ("ref").
// Where we cannot safely emit DDL we leave a `-- TODO:` comment.
//
// Statements are grouped by phase so that drops happen before creates, and
// objects with cross-references (FKs, views, policies) are torn down/created
// in a sane order. Within a phase we keep input order — diff already sorts by
// (schema, name).
pub fn sql(changes: &[Change]) -> String {
    let mut buckets = Buckets::default();
    for c in changes {
        bucket(c, &mut buckets);
    }

    let mut out = String::new();
    let mut first = true;
    for stmt in buckets.into_ordered() {
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(&stmt);
        if !stmt.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[derive(Default)]
struct Buckets {
    // Drop order (most dependent first).
    drop_policies: Vec<String>,
    drop_triggers: Vec<String>,
    drop_views: Vec<ViewSlot>,
    drop_constraints: Vec<String>,
    drop_indexes: Vec<String>,
    drop_columns: Vec<String>,
    drop_tables: Vec<String>,
    drop_functions: Vec<String>,
    drop_sequences: Vec<String>,
    drop_types: Vec<String>,
    drop_schemas: Vec<String>,
    drop_extensions: Vec<String>,

    // Create order (least dependent first).
    create_extensions: Vec<String>,
    create_schemas: Vec<String>,
    create_types: Vec<String>,
    create_sequences: Vec<String>,
    create_functions: Vec<String>,
    create_tables: Vec<String>,
    create_columns: Vec<String>,
    create_constraints: Vec<String>,
    create_indexes: Vec<String>,
    create_views: Vec<ViewSlot>,
    create_triggers: Vec<String>,
    create_policies: Vec<String>,

    // Mixed alterations.
    rls: Vec<String>,
    column_changes: Vec<String>,
    sequence_changes: Vec<String>,
    extension_changes: Vec<String>,
    other_changes: Vec<String>,
}

impl Buckets {
    fn into_ordered(self) -> Vec<String> {
        let mut v = Vec::new();
        v.extend(self.drop_policies);
        v.extend(self.drop_triggers);
        // Drops go in *reverse* topo order: an upstream view (the one others
        // SELECT from) must outlive its dependents during the drop phase.
        v.extend(topo_sort_views(&self.drop_views, true));
        v.extend(self.drop_constraints);
        v.extend(self.drop_indexes);
        v.extend(self.drop_columns);
        v.extend(self.drop_tables);
        v.extend(self.drop_functions);
        v.extend(self.drop_sequences);
        // drop_types deferred until *after* column_changes (see below) so a
        // type a column is being retyped off can still be referenced when
        // DROP TYPE runs.
        v.extend(self.drop_schemas);
        v.extend(self.drop_extensions);

        v.extend(self.create_extensions);
        v.extend(self.create_schemas);
        v.extend(self.create_types);
        v.extend(self.create_sequences);
        v.extend(self.create_functions);
        v.extend(self.create_tables);
        v.extend(self.create_columns);
        v.extend(self.create_constraints);
        v.extend(self.create_indexes);
        // Creates go in topo order: a view's dependencies must already exist
        // by the time we run its CREATE.
        v.extend(topo_sort_views(&self.create_views, false));
        v.extend(self.create_triggers);
        v.extend(self.create_policies);

        v.extend(self.rls);
        // ALTER TYPE … ADD VALUE lives in other_changes; it must commit
        // before any column_change that actually stores the new value.
        v.extend(self.other_changes);
        v.extend(self.column_changes);
        v.extend(self.sequence_changes);
        v.extend(self.extension_changes);
        // Now that column_changes have moved any consumers off the doomed
        // types, dropping them is safe.
        v.extend(self.drop_types);
        v
    }
}

/// Topologically sort view slots by their `depends_on` edges, so each view
/// appears after all its dependencies. Pass `reverse=true` for the drop pass
/// to invert the order. Dependencies pointing outside the slot set (e.g. to
/// base tables or to views unaffected by this diff) are ignored — only
/// edges between slots in this batch matter for ordering.
///
/// Cycles can't normally exist between Postgres views (the catalog rejects
/// them) but we tolerate them by emitting any unresolved tail in input
/// order, so a malformed input doesn't crash the emitter.
fn topo_sort_views(slots: &[ViewSlot], reverse: bool) -> Vec<String> {
    if slots.len() < 2 {
        return slots.iter().map(|s| s.sql.clone()).collect();
    }

    let names: BTreeSet<&str> = slots.iter().map(|s| s.qual.as_str()).collect();
    let by_name: BTreeMap<&str, &ViewSlot> = slots.iter().map(|s| (s.qual.as_str(), s)).collect();

    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let mut on_stack: BTreeSet<&str> = BTreeSet::new();
    let mut order: Vec<&ViewSlot> = Vec::with_capacity(slots.len());

    fn visit<'a>(
        node: &'a str,
        by_name: &BTreeMap<&'a str, &'a ViewSlot>,
        names: &BTreeSet<&'a str>,
        visited: &mut BTreeSet<&'a str>,
        on_stack: &mut BTreeSet<&'a str>,
        order: &mut Vec<&'a ViewSlot>,
    ) {
        if visited.contains(node) || on_stack.contains(node) {
            return;
        }
        on_stack.insert(node);
        if let Some(slot) = by_name.get(node) {
            for dep in &slot.depends_on {
                let dep_str = dep.as_str();
                if names.contains(dep_str) {
                    visit(dep_str, by_name, names, visited, on_stack, order);
                }
            }
            order.push(slot);
        }
        on_stack.remove(node);
        visited.insert(node);
    }

    // Stable input order — for views with no dependency relationship to each
    // other, the original (alphabetical) input order is preserved.
    for slot in slots {
        visit(
            slot.qual.as_str(),
            &by_name,
            &names,
            &mut visited,
            &mut on_stack,
            &mut order,
        );
    }

    if reverse {
        order.reverse();
    }
    order.iter().map(|s| s.sql.clone()).collect()
}

fn bucket(c: &Change, b: &mut Buckets) {
    match c {
        Change::SchemaAdded { name } => {
            b.create_schemas.push(format!("CREATE SCHEMA IF NOT EXISTS {};", quote_ident(name)));
        }
        Change::SchemaRemoved { name } => {
            b.drop_schemas.push(format!("DROP SCHEMA {} CASCADE;", quote_ident(name)));
        }

        Change::ExtensionAdded { name, extension } => {
            b.create_extensions.push(format!(
                "CREATE EXTENSION IF NOT EXISTS {} WITH SCHEMA {} VERSION {};",
                quote_ident(name),
                quote_ident(&extension.schema),
                quote_literal(&extension.version),
            ));
        }
        Change::ExtensionRemoved { name } => {
            b.drop_extensions.push(format!("DROP EXTENSION {};", quote_ident(name)));
        }
        Change::ExtensionChanged { name, after, .. } => {
            b.extension_changes.push(format!(
                "ALTER EXTENSION {} UPDATE TO {};",
                quote_ident(name),
                quote_literal(&after.version),
            ));
        }

        Change::TableAdded { qual, table } => {
            b.create_tables.push(emit_create_table(qual, table));
            // Indexes / triggers / policies on a freshly-created table are
            // not in `Change::IndexAdded` etc. — they're inside the
            // TableAdded payload. Emit them in the right phase.
            for (name, idx) in &table.indexes {
                b.create_indexes.push(format!("{};", idx.definition));
                let _ = name;
            }
            for (name, con) in &table.constraints {
                b.create_constraints.push(format!(
                    "ALTER TABLE {} ADD CONSTRAINT {} {};",
                    qual_ident(qual),
                    quote_ident(name),
                    con.definition,
                ));
            }
            for (_, trg) in &table.triggers {
                b.create_triggers.push(format!("{};", trg.definition));
            }
            for (name, pol) in &table.policies {
                b.create_policies.push(emit_create_policy(qual, name, pol));
            }
            if table.rls_enabled {
                b.rls.push(format!(
                    "ALTER TABLE {} ENABLE ROW LEVEL SECURITY;",
                    qual_ident(qual),
                ));
            }
        }
        Change::TableRemoved { qual } => {
            b.drop_tables
                .push(format!("DROP TABLE {};", qual_ident(qual)));
        }

        Change::ColumnAdded { table, column } => {
            b.create_columns.push(format!(
                "ALTER TABLE {} ADD COLUMN {};",
                qual_ident(table),
                column_decl(column),
            ));
        }
        Change::ColumnRemoved { table, name } => {
            b.drop_columns.push(format!(
                "ALTER TABLE {} DROP COLUMN {};",
                qual_ident(table),
                quote_ident(name),
            ));
        }
        Change::ColumnChanged { table, name, before, after } => {
            b.column_changes
                .push(emit_column_change(table, name, before, after));
        }

        Change::PrimaryKeyAdded { table, index } => {
            b.create_constraints.push(format!(
                "ALTER TABLE {} ADD {};",
                qual_ident(table),
                pk_clause(&index.definition),
            ));
        }
        Change::PrimaryKeyRemoved { table, .. } => {
            // The constraint name isn't in the Index struct; PG defaults to
            // `<table>_pkey`. Emit a best-effort drop with that convention.
            b.drop_constraints.push(format!(
                "ALTER TABLE {} DROP CONSTRAINT {};",
                qual_ident(table),
                quote_ident(&format!("{}_pkey", table.name)),
            ));
        }
        Change::PrimaryKeyChanged { table, after, .. } => {
            b.drop_constraints.push(format!(
                "ALTER TABLE {} DROP CONSTRAINT {};",
                qual_ident(table),
                quote_ident(&format!("{}_pkey", table.name)),
            ));
            b.create_constraints.push(format!(
                "ALTER TABLE {} ADD {};",
                qual_ident(table),
                pk_clause(&after.definition),
            ));
        }

        Change::IndexAdded { index, .. } => {
            b.create_indexes.push(format!("{};", index.definition));
        }
        Change::IndexRemoved { table, name } => {
            b.drop_indexes.push(format!(
                "DROP INDEX {}.{};",
                quote_ident(&table.schema),
                quote_ident(name),
            ));
        }
        Change::IndexChanged { table, name, after, .. } => {
            b.drop_indexes.push(format!(
                "DROP INDEX {}.{};",
                quote_ident(&table.schema),
                quote_ident(name),
            ));
            b.create_indexes.push(format!("{};", after.definition));
        }

        Change::ConstraintAdded { table, name, constraint } => {
            b.create_constraints.push(format!(
                "ALTER TABLE {} ADD CONSTRAINT {} {};",
                qual_ident(table),
                quote_ident(name),
                constraint.definition,
            ));
        }
        Change::ConstraintRemoved { table, name } => {
            b.drop_constraints.push(format!(
                "ALTER TABLE {} DROP CONSTRAINT {};",
                qual_ident(table),
                quote_ident(name),
            ));
        }
        Change::ConstraintChanged { table, name, after, .. } => {
            b.drop_constraints.push(format!(
                "ALTER TABLE {} DROP CONSTRAINT {};",
                qual_ident(table),
                quote_ident(name),
            ));
            b.create_constraints.push(format!(
                "ALTER TABLE {} ADD CONSTRAINT {} {};",
                qual_ident(table),
                quote_ident(name),
                after.definition,
            ));
        }

        Change::RlsEnabled { table } => {
            b.rls.push(format!(
                "ALTER TABLE {} ENABLE ROW LEVEL SECURITY;",
                qual_ident(table),
            ));
        }
        Change::RlsDisabled { table } => {
            b.rls.push(format!(
                "ALTER TABLE {} DISABLE ROW LEVEL SECURITY;",
                qual_ident(table),
            ));
        }

        Change::ViewAdded { qual, materialized, view } => {
            let kw = if *materialized { "MATERIALIZED VIEW" } else { "VIEW" };
            b.create_views.push(ViewSlot {
                qual: format!("{}.{}", qual.schema, qual.name),
                depends_on: view.depends_on.clone(),
                sql: format!(
                    "CREATE {} {} AS\n{}",
                    kw,
                    qual_ident(qual),
                    ensure_terminated(&view.definition),
                ),
            });
        }
        Change::ViewRemoved { qual, materialized, depends_on } => {
            let kw = if *materialized { "MATERIALIZED VIEW" } else { "VIEW" };
            b.drop_views.push(ViewSlot {
                qual: format!("{}.{}", qual.schema, qual.name),
                depends_on: depends_on.clone(),
                sql: format!("DROP {} {};", kw, qual_ident(qual)),
            });
        }
        Change::ViewChanged { qual, materialized, before, after } => {
            let kw = if *materialized { "MATERIALIZED VIEW" } else { "VIEW" };
            let qual_str = format!("{}.{}", qual.schema, qual.name);
            b.drop_views.push(ViewSlot {
                qual: qual_str.clone(),
                depends_on: before.depends_on.clone(),
                sql: format!("DROP {} {};", kw, qual_ident(qual)),
            });
            b.create_views.push(ViewSlot {
                qual: qual_str,
                depends_on: after.depends_on.clone(),
                sql: format!(
                    "CREATE {} {} AS\n{}",
                    kw,
                    qual_ident(qual),
                    ensure_terminated(&after.definition),
                ),
            });
        }

        Change::SequenceAdded { qual, sequence } => {
            b.create_sequences.push(emit_create_sequence(qual, sequence));
        }
        Change::SequenceRemoved { qual } => {
            b.drop_sequences
                .push(format!("DROP SEQUENCE {};", qual_ident(qual)));
        }
        Change::SequenceChanged { qual, before, after } => {
            b.sequence_changes
                .push(emit_sequence_change(qual, before, after));
        }

        Change::TypeAdded { qual, user_type } => {
            b.create_types.push(emit_create_type(qual, user_type));
        }
        Change::TypeRemoved { qual } => {
            b.drop_types.push(format!("DROP TYPE {};", qual_ident(qual)));
        }
        Change::TypeChanged { qual, before, after } => {
            b.other_changes.push(emit_type_change(qual, before, after));
        }

        Change::TriggerAdded { trigger, .. } => {
            b.create_triggers
                .push(ensure_terminated(&trigger.definition));
        }
        Change::TriggerRemoved { table, name } => {
            b.drop_triggers.push(format!(
                "DROP TRIGGER {} ON {};",
                quote_ident(name),
                qual_ident(table),
            ));
        }
        Change::TriggerChanged { table, name, after, .. } => {
            b.drop_triggers.push(format!(
                "DROP TRIGGER {} ON {};",
                quote_ident(name),
                qual_ident(table),
            ));
            b.create_triggers
                .push(ensure_terminated(&after.definition));
        }

        Change::PolicyAdded { table, name, policy } => {
            b.create_policies
                .push(emit_create_policy(table, name, policy));
        }
        Change::PolicyRemoved { table, name } => {
            b.drop_policies.push(format!(
                "DROP POLICY {} ON {};",
                quote_ident(name),
                qual_ident(table),
            ));
        }
        Change::PolicyChanged { table, name, after, .. } => {
            b.drop_policies.push(format!(
                "DROP POLICY {} ON {};",
                quote_ident(name),
                qual_ident(table),
            ));
            b.create_policies
                .push(emit_create_policy(table, name, after));
        }

        Change::FunctionAdded { function, .. } => {
            b.create_functions
                .push(ensure_terminated(&function.definition));
        }
        Change::FunctionRemoved { qual } => {
            // `qual.name` already includes the argument signature
            // (e.g. `my_fn(integer, text)`) so DROP FUNCTION can pinpoint
            // the right overload.
            b.drop_functions.push(format!(
                "DROP FUNCTION {}.{};",
                quote_ident(&qual.schema),
                qual.name,
            ));
        }
        Change::FunctionChanged { after, .. } => {
            // pg_get_functiondef emits CREATE OR REPLACE, so this works as
            // an in-place update for everything except signature changes.
            b.other_changes
                .push(ensure_terminated(&after.definition));
        }

        Change::PartitionByChanged { table, .. } => {
            // PARTITION BY is fixed at CREATE TABLE — Postgres has no ALTER
            // TABLE … PARTITION BY. Surfacing this as a TODO is the honest
            // answer: a strategy/key change requires a manual rebuild.
            b.other_changes.push(format!(
                "-- TODO: PARTITION BY clause changed on {} — drop+recreate the table to apply",
                qual_ident(table),
            ));
        }
        Change::PartitionOfChanged { table, before, after } => {
            // ATTACH/DETACH is reversible without rebuilding, so emit those
            // when we can. A bound change on the same parent collapses to
            // DETACH then ATTACH with the new bound.
            match (before, after) {
                (None, Some(info)) => {
                    b.other_changes.push(format!(
                        "ALTER TABLE {} ATTACH PARTITION {} {};",
                        info.parent,
                        qual_ident(table),
                        info.bound,
                    ));
                }
                (Some(info), None) => {
                    b.other_changes.push(format!(
                        "ALTER TABLE {} DETACH PARTITION {};",
                        info.parent,
                        qual_ident(table),
                    ));
                }
                (Some(b_info), Some(a_info)) => {
                    b.other_changes.push(format!(
                        "ALTER TABLE {} DETACH PARTITION {};",
                        b_info.parent,
                        qual_ident(table),
                    ));
                    b.other_changes.push(format!(
                        "ALTER TABLE {} ATTACH PARTITION {} {};",
                        a_info.parent,
                        qual_ident(table),
                        a_info.bound,
                    ));
                }
                (None, None) => {}
            }
        }
    }
}

fn emit_create_table(qual: &QualifiedName, table: &Table) -> String {
    // Partition children inherit columns from the parent — emit the short
    // PARTITION OF form so we don't have to re-list (and risk drifting from)
    // the parent's column definitions. PRIMARY KEY/indexes/etc on the child
    // get emitted separately by the TableAdded handler.
    if let Some(info) = &table.partition_of {
        return format!(
            "CREATE TABLE {} PARTITION OF {} {};",
            qual_ident(qual),
            info.parent,
            info.bound,
        );
    }

    let mut s = String::new();
    let _ = writeln!(s, "CREATE TABLE {} (", qual_ident(qual));
    let mut parts: Vec<String> = table.columns.iter().map(|c| format!("    {}", column_decl(c))).collect();
    if let Some(pk) = &table.primary_key {
        parts.push(format!("    {}", pk_clause(&pk.definition)));
    }
    s.push_str(&parts.join(",\n"));
    s.push_str("\n)");
    if let Some(pb) = &table.partition_by {
        let _ = write!(s, " PARTITION BY {} {}", pb.strategy, pb.key);
    }
    s.push(';');
    s
}

fn column_decl(c: &Column) -> String {
    let mut s = format!("{} {}", quote_ident(&c.name), c.data_type);
    if !c.nullable {
        s.push_str(" NOT NULL");
    }
    if let Some(d) = &c.default {
        s.push_str(&format!(" DEFAULT {d}"));
    }
    if let Some(id) = &c.identity {
        let kw = match id {
            Identity::Always => "ALWAYS",
            Identity::ByDefault => "BY DEFAULT",
        };
        s.push_str(&format!(" GENERATED {kw} AS IDENTITY"));
    }
    if let Some(g) = &c.generated {
        s.push_str(&format!(" GENERATED ALWAYS AS ({g}) STORED"));
    }
    if let Some(coll) = &c.collation {
        s.push_str(&format!(" COLLATE {}", quote_ident(coll)));
    }
    s
}

fn emit_column_change(
    table: &QualifiedName,
    name: &str,
    before: &Column,
    after: &Column,
) -> String {
    let mut stmts = Vec::new();
    if before.data_type != after.data_type {
        stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} TYPE {};",
            qual_ident(table),
            quote_ident(name),
            after.data_type,
        ));
    }
    if before.nullable != after.nullable {
        let action = if after.nullable { "DROP NOT NULL" } else { "SET NOT NULL" };
        stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} {action};",
            qual_ident(table),
            quote_ident(name),
        ));
    }
    if before.default != after.default {
        match &after.default {
            Some(d) => stmts.push(format!(
                "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {};",
                qual_ident(table),
                quote_ident(name),
                d,
            )),
            None => stmts.push(format!(
                "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT;",
                qual_ident(table),
                quote_ident(name),
            )),
        }
    }
    if before.identity != after.identity {
        match (&before.identity, &after.identity) {
            (Some(_), None) => stmts.push(format!(
                "ALTER TABLE {} ALTER COLUMN {} DROP IDENTITY;",
                qual_ident(table),
                quote_ident(name),
            )),
            (None, Some(id)) => {
                let kw = match id {
                    Identity::Always => "ALWAYS",
                    Identity::ByDefault => "BY DEFAULT",
                };
                stmts.push(format!(
                    "ALTER TABLE {} ALTER COLUMN {} ADD GENERATED {kw} AS IDENTITY;",
                    qual_ident(table),
                    quote_ident(name),
                ));
            }
            _ => stmts.push(format!(
                "-- TODO: change IDENTITY mode on {}.{}",
                qual_ident(table),
                quote_ident(name),
            )),
        }
    }
    if before.generated != after.generated {
        stmts.push(format!(
            "-- TODO: GENERATED expression changed on {}.{} (drop+add column required in PG)",
            qual_ident(table),
            quote_ident(name),
        ));
    }
    if stmts.is_empty() {
        format!(
            "-- no-op column change on {}.{}",
            qual_ident(table),
            quote_ident(name),
        )
    } else {
        stmts.join("\n")
    }
}

fn emit_create_sequence(qual: &QualifiedName, s: &Sequence) -> String {
    let mut sql = format!("CREATE SEQUENCE {} AS {}", qual_ident(qual), s.data_type);
    let _ = write!(sql, " INCREMENT BY {}", s.increment);
    let _ = write!(sql, " MINVALUE {}", s.min_value);
    let _ = write!(sql, " MAXVALUE {}", s.max_value);
    let _ = write!(sql, " START WITH {}", s.start);
    let _ = write!(sql, " CACHE {}", s.cache);
    if s.cycle {
        sql.push_str(" CYCLE");
    } else {
        sql.push_str(" NO CYCLE");
    }
    if let Some(owner) = &s.owned_by {
        let _ = write!(sql, " OWNED BY {owner}");
    }
    sql.push(';');
    sql
}

fn emit_sequence_change(qual: &QualifiedName, before: &Sequence, after: &Sequence) -> String {
    let mut clauses: Vec<String> = Vec::new();
    if before.data_type != after.data_type {
        clauses.push(format!("AS {}", after.data_type));
    }
    if before.increment != after.increment {
        clauses.push(format!("INCREMENT BY {}", after.increment));
    }
    if before.min_value != after.min_value {
        clauses.push(format!("MINVALUE {}", after.min_value));
    }
    if before.max_value != after.max_value {
        clauses.push(format!("MAXVALUE {}", after.max_value));
    }
    if before.start != after.start {
        clauses.push(format!("START WITH {}", after.start));
    }
    if before.cache != after.cache {
        clauses.push(format!("CACHE {}", after.cache));
    }
    if before.cycle != after.cycle {
        clauses.push(if after.cycle { "CYCLE".into() } else { "NO CYCLE".into() });
    }
    if before.owned_by != after.owned_by {
        match &after.owned_by {
            Some(o) => clauses.push(format!("OWNED BY {o}")),
            None => clauses.push("OWNED BY NONE".into()),
        }
    }
    if clauses.is_empty() {
        format!("-- no-op sequence change on {}", qual_ident(qual))
    } else {
        format!("ALTER SEQUENCE {} {};", qual_ident(qual), clauses.join(" "))
    }
}

fn emit_create_type(qual: &QualifiedName, t: &UserType) -> String {
    match t {
        UserType::Enum { values } => {
            let vs: Vec<String> = values.iter().map(|v| quote_literal(v)).collect();
            format!(
                "CREATE TYPE {} AS ENUM ({});",
                qual_ident(qual),
                vs.join(", ")
            )
        }
        UserType::Composite { fields } => {
            let fs: Vec<String> = fields
                .iter()
                .map(|(n, t)| format!("    {} {}", quote_ident(n), t))
                .collect();
            format!(
                "CREATE TYPE {} AS (\n{}\n);",
                qual_ident(qual),
                fs.join(",\n"),
            )
        }
        UserType::Domain { base_type, definition } => {
            // pg_get_constraintdef already emits the constraint clauses we
            // need; `definition` contains the full check chain or is empty.
            if definition.is_empty() {
                format!(
                    "CREATE DOMAIN {} AS {};",
                    qual_ident(qual),
                    base_type,
                )
            } else {
                format!(
                    "CREATE DOMAIN {} AS {} {};",
                    qual_ident(qual),
                    base_type,
                    definition,
                )
            }
        }
        UserType::Range { subtype, definition } => {
            if definition.is_empty() {
                format!(
                    "CREATE TYPE {} AS RANGE (SUBTYPE = {});",
                    qual_ident(qual),
                    subtype,
                )
            } else {
                format!(
                    "CREATE TYPE {} AS RANGE ({});",
                    qual_ident(qual),
                    definition,
                )
            }
        }
    }
}

fn emit_type_change(qual: &QualifiedName, before: &UserType, after: &UserType) -> String {
    match (before, after) {
        (UserType::Enum { values: bv }, UserType::Enum { values: av }) => {
            let mut stmts = Vec::new();
            for v in av {
                if !bv.contains(v) {
                    stmts.push(format!(
                        "ALTER TYPE {} ADD VALUE IF NOT EXISTS {};",
                        qual_ident(qual),
                        quote_literal(v),
                    ));
                }
            }
            for v in bv {
                if !av.contains(v) {
                    stmts.push(format!(
                        "-- TODO: enum value {} removed from {} — PG cannot drop enum values",
                        quote_literal(v),
                        qual_ident(qual),
                    ));
                }
            }
            if stmts.is_empty() {
                format!("-- enum {} reordered (no SQL needed)", qual_ident(qual))
            } else {
                stmts.join("\n")
            }
        }
        _ => format!(
            "-- TODO: type change on {} requires manual migration",
            qual_ident(qual),
        ),
    }
}

fn emit_create_policy(
    table: &QualifiedName,
    name: &str,
    p: &crate::model::Policy,
) -> String {
    let mut s = format!(
        "CREATE POLICY {} ON {}",
        quote_ident(name),
        qual_ident(table),
    );
    if !p.permissive {
        s.push_str(" AS RESTRICTIVE");
    }
    if !p.command.eq_ignore_ascii_case("ALL") {
        let _ = write!(s, " FOR {}", p.command.to_uppercase());
    }
    if !p.roles.is_empty() {
        let roles: Vec<String> = p
            .roles
            .iter()
            .map(|r| if r.eq_ignore_ascii_case("PUBLIC") { "PUBLIC".to_string() } else { quote_ident(r) })
            .collect();
        let _ = write!(s, " TO {}", roles.join(", "));
    }
    if let Some(q) = &p.qual {
        let _ = write!(s, " USING ({q})");
    }
    if let Some(wc) = &p.with_check {
        let _ = write!(s, " WITH CHECK ({wc})");
    }
    s.push(';');
    s
}

// Extracts the parenthesised body of a PRIMARY KEY clause from a constraintdef
// like `PRIMARY KEY (id, name) INCLUDE (...)`. Returns `PRIMARY KEY (...)` —
// just hands back the whole thing since pg_get_constraintdef is already
// canonical.
fn pk_clause(def: &str) -> String {
    def.trim().to_string()
}

fn qual_ident(q: &QualifiedName) -> String {
    format!("{}.{}", quote_ident(&q.schema), quote_ident(&q.name))
}

fn quote_ident(s: &str) -> String {
    let needs_quoting = !s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        || s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true)
        || RESERVED.contains(&s);
    if needs_quoting {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn ensure_terminated(s: &str) -> String {
    let t = s.trim_end();
    if t.ends_with(';') {
        t.to_string()
    } else {
        format!("{t};")
    }
}

// Common SQL reserved words. Not exhaustive — Postgres has a long list — but
// covers the ones likely to appear as identifiers in catalog output.
const RESERVED: &[&str] = &[
    "all", "and", "any", "as", "asc", "case", "check", "collate", "column",
    "constraint", "create", "current_date", "current_time", "current_timestamp",
    "current_user", "default", "deferrable", "desc", "distinct", "do", "else",
    "end", "except", "false", "for", "foreign", "from", "grant", "group",
    "having", "in", "initially", "intersect", "into", "is", "join", "leading",
    "limit", "localtime", "localtimestamp", "not", "null", "offset", "on",
    "only", "or", "order", "placing", "primary", "references", "returning",
    "select", "session_user", "some", "table", "then", "to", "trailing",
    "true", "union", "unique", "user", "using", "when", "where", "window",
    "with",
];
