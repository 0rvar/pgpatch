use crate::catalog::policies::normalize_roles;
use crate::model::{
    Column, Constraint, Extension, Function, Index, Namespace, PartitionBy, PartitionInfo, Policy,
    QualifiedName, Schema, Sequence, Table, Trigger, UserType, View,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    SchemaAdded {
        name: String,
    },
    SchemaRemoved {
        name: String,
    },

    TableAdded {
        qual: QualifiedName,
        table: Table,
    },
    TableRemoved {
        qual: QualifiedName,
    },

    ColumnAdded {
        table: QualifiedName,
        column: Column,
    },
    ColumnRemoved {
        table: QualifiedName,
        name: String,
    },
    ColumnChanged {
        table: QualifiedName,
        name: String,
        before: Column,
        after: Column,
    },

    PrimaryKeyAdded {
        table: QualifiedName,
        index: Index,
    },
    PrimaryKeyRemoved {
        table: QualifiedName,
        index: Index,
    },
    PrimaryKeyChanged {
        table: QualifiedName,
        before: Index,
        after: Index,
    },

    IndexAdded {
        table: QualifiedName,
        name: String,
        index: Index,
    },
    IndexRemoved {
        table: QualifiedName,
        name: String,
    },
    IndexChanged {
        table: QualifiedName,
        name: String,
        before: Index,
        after: Index,
    },

    ConstraintAdded {
        table: QualifiedName,
        name: String,
        constraint: Constraint,
    },
    ConstraintRemoved {
        table: QualifiedName,
        name: String,
    },
    ConstraintChanged {
        table: QualifiedName,
        name: String,
        before: Constraint,
        after: Constraint,
    },

    RlsEnabled {
        table: QualifiedName,
    },
    RlsDisabled {
        table: QualifiedName,
    },

    ViewAdded {
        qual: QualifiedName,
        materialized: bool,
        view: View,
    },
    ViewRemoved {
        qual: QualifiedName,
        materialized: bool,
        depends_on: Vec<String>,
    },
    ViewChanged {
        qual: QualifiedName,
        materialized: bool,
        before: View,
        after: View,
    },

    SequenceAdded {
        qual: QualifiedName,
        sequence: Sequence,
    },
    SequenceRemoved {
        qual: QualifiedName,
    },
    SequenceChanged {
        qual: QualifiedName,
        before: Sequence,
        after: Sequence,
    },

    TypeAdded {
        qual: QualifiedName,
        user_type: UserType,
    },
    TypeRemoved {
        qual: QualifiedName,
    },
    TypeChanged {
        qual: QualifiedName,
        before: UserType,
        after: UserType,
    },

    TriggerAdded {
        table: QualifiedName,
        name: String,
        trigger: Trigger,
    },
    TriggerRemoved {
        table: QualifiedName,
        name: String,
    },
    TriggerChanged {
        table: QualifiedName,
        name: String,
        before: Trigger,
        after: Trigger,
    },

    PolicyAdded {
        table: QualifiedName,
        name: String,
        policy: Policy,
    },
    PolicyRemoved {
        table: QualifiedName,
        name: String,
    },
    PolicyChanged {
        table: QualifiedName,
        name: String,
        before: Policy,
        after: Policy,
    },

    FunctionAdded {
        qual: QualifiedName,
        function: Function,
    },
    FunctionRemoved {
        qual: QualifiedName,
        function: Function,
    },
    FunctionChanged {
        qual: QualifiedName,
        before: Function,
        after: Function,
    },
    FunctionGrantsChanged {
        qual: QualifiedName,
        before: Function,
        after: Function,
    },

    ExtensionAdded {
        name: String,
        extension: Extension,
    },
    ExtensionRemoved {
        name: String,
    },
    ExtensionChanged {
        name: String,
        before: Extension,
        after: Extension,
    },

    PartitionByChanged {
        table: QualifiedName,
        before: Option<PartitionBy>,
        after: Option<PartitionBy>,
    },
    PartitionOfChanged {
        table: QualifiedName,
        before: Option<PartitionInfo>,
        after: Option<PartitionInfo>,
    },
}

pub fn diff(left: &Schema, right: &Schema) -> Vec<Change> {
    let mut out = Vec::new();

    diff_extensions(&left.extensions, &right.extensions, &mut out);

    for name in left.schemas.keys() {
        if !right.schemas.contains_key(name) {
            out.push(Change::SchemaRemoved { name: name.clone() });
        }
    }
    for name in right.schemas.keys() {
        if !left.schemas.contains_key(name) {
            out.push(Change::SchemaAdded { name: name.clone() });
        }
    }

    for (sname, lns) in &left.schemas {
        let Some(rns) = right.schemas.get(sname) else {
            continue;
        };
        diff_namespace(sname, lns, rns, &mut out);
    }

    suppress_partition_changes_covered_by_parent(left, right, out)
}

/// Changes on a partition that its parent already carries out.
///
/// Columns are inherited from the parent: Postgres rejects `ADD COLUMN` /
/// `DROP COLUMN` on a partition outright, and an `ALTER COLUMN` on the parent
/// recurses into every partition. The per-table column diff still sees the
/// inherited columns on both tables, so a parent change produces a mirrored
/// change on each partition. Applying the mirror after the parent either
/// errors ("cannot drop inherited column", or the column is already gone) or
/// is a no-op, so drop it here.
///
/// A table only counts as a partition when both snapshots agree on its
/// parent and that parent is in the snapshot. A table being attached or
/// detached in this diff is standalone at one end and needs its own column
/// statements there; a parent outside the snapshot cannot cover anything, so
/// its partitions' changes stay in and fail loudly at apply time instead of
/// vanishing from the diff.
///
/// A column change on a partition is covered only when the parent's change
/// leaves the column in exactly the state the partition wants. Otherwise the
/// partition-local remainder (typically a per-partition default) is kept and
/// moved after the parent's changes, since emit preserves diff order within a
/// phase and the parent's recursive `ALTER` would otherwise clobber it. A
/// column the parent adds with a different default becomes an `ALTER COLUMN`
/// on the partition instead of the impossible `ADD COLUMN`.
///
/// `DROP TABLE` on a partitioned parent takes its partitions with it, so a
/// partition's own `TableRemoved` is dropped when the parent is removed too.
/// Removing a partition alone (the parent stays) is kept.
fn suppress_partition_changes_covered_by_parent(
    left: &Schema,
    right: &Schema,
    changes: Vec<Change>,
) -> Vec<Change> {
    fn table<'a>(schema: &'a Schema, flat: &str) -> Option<&'a Table> {
        let (sname, tname) = flat.split_once('.')?;
        schema.schemas.get(sname)?.tables.get(tname)
    }
    fn parent_of<'a>(schema: &'a Schema, flat: &str) -> Option<&'a str> {
        table(schema, flat)?
            .partition_of
            .as_ref()
            .map(|p| p.parent.as_str())
    }
    // Parent of a partition that is one on both sides, with the parent present.
    let stable_parent = |qual: &QualifiedName| -> Option<String> {
        let flat = qual.to_string();
        let parent = parent_of(right, &flat)?;
        if parent_of(left, &flat) != Some(parent) {
            return None;
        }
        (table(left, parent).is_some() && table(right, parent).is_some())
            .then(|| parent.to_string())
    };

    let mut removed_tables: BTreeSet<String> = BTreeSet::new();
    let mut removed_columns: BTreeSet<(String, String)> = BTreeSet::new();
    // (table, column) -> the column as it looks after the change.
    let mut column_after: BTreeMap<(String, String), Column> = BTreeMap::new();
    for c in &changes {
        match c {
            Change::TableRemoved { qual } => {
                removed_tables.insert(qual.to_string());
            }
            Change::ColumnRemoved { table, name } => {
                removed_columns.insert((table.to_string(), name.clone()));
            }
            Change::ColumnAdded { table, column } => {
                column_after.insert((table.to_string(), column.name.clone()), column.clone());
            }
            Change::ColumnChanged {
                table, name, after, ..
            } => {
                column_after.insert((table.to_string(), name.clone()), after.clone());
            }
            _ => {}
        }
    }
    if removed_tables.is_empty() && removed_columns.is_empty() && column_after.is_empty() {
        return changes;
    }

    let mut out = Vec::with_capacity(changes.len());
    let mut partition_local = Vec::new();
    for c in changes {
        match c {
            Change::TableRemoved { ref qual } => {
                let covered = parent_of(left, &qual.to_string())
                    .is_some_and(|parent| removed_tables.contains(parent));
                if !covered {
                    out.push(c);
                }
            }
            Change::ColumnRemoved {
                ref table,
                ref name,
            } => {
                let covered = stable_parent(table)
                    .is_some_and(|parent| removed_columns.contains(&(parent, name.clone())));
                if !covered {
                    out.push(c);
                }
            }
            Change::ColumnAdded {
                ref table,
                ref column,
            } => {
                let Some(parent) = stable_parent(table) else {
                    out.push(c);
                    continue;
                };
                match column_after.get(&(parent, column.name.clone())) {
                    Some(inherited) if inherited == column => {}
                    Some(inherited) => partition_local.push(Change::ColumnChanged {
                        table: table.clone(),
                        name: column.name.clone(),
                        before: inherited.clone(),
                        after: column.clone(),
                    }),
                    None => out.push(c),
                }
            }
            Change::ColumnChanged {
                ref table,
                ref name,
                ref after,
                ..
            } => match stable_parent(table) {
                None => out.push(c),
                Some(parent) => match column_after.get(&(parent, name.clone())) {
                    Some(inherited) if inherited == after => {}
                    Some(_) => partition_local.push(c),
                    None => out.push(c),
                },
            },
            _ => out.push(c),
        }
    }
    out.extend(partition_local);
    out
}

fn diff_extensions(
    left: &BTreeMap<String, Extension>,
    right: &BTreeMap<String, Extension>,
    out: &mut Vec<Change>,
) {
    for (name, l) in left {
        match right.get(name) {
            None => out.push(Change::ExtensionRemoved { name: name.clone() }),
            Some(r) if l != r => out.push(Change::ExtensionChanged {
                name: name.clone(),
                before: l.clone(),
                after: r.clone(),
            }),
            _ => {}
        }
    }
    for (name, r) in right {
        if !left.contains_key(name) {
            out.push(Change::ExtensionAdded {
                name: name.clone(),
                extension: r.clone(),
            });
        }
    }
}

fn diff_namespace(sname: &str, left: &Namespace, right: &Namespace, out: &mut Vec<Change>) {
    for (tname, ltable) in &left.tables {
        let qual = QualifiedName::new(sname, tname);
        match right.tables.get(tname) {
            None => out.push(Change::TableRemoved { qual }),
            Some(rtable) => diff_table(&qual, ltable, rtable, out),
        }
    }
    for (tname, rtable) in &right.tables {
        if !left.tables.contains_key(tname) {
            out.push(Change::TableAdded {
                qual: QualifiedName::new(sname, tname),
                table: rtable.clone(),
            });
        }
    }

    diff_view_bucket(sname, &left.views, &right.views, false, out);
    diff_view_bucket(
        sname,
        &left.materialized_views,
        &right.materialized_views,
        true,
        out,
    );
    diff_sequences(sname, &left.sequences, &right.sequences, out);
    diff_types(sname, &left.types, &right.types, out);
    diff_functions(sname, &left.functions, &right.functions, out);
}

fn diff_functions(
    sname: &str,
    left: &BTreeMap<String, Function>,
    right: &BTreeMap<String, Function>,
    out: &mut Vec<Change>,
) {
    // Functions match on their identity signature (the `name(identity_args)`
    // map key). The result type never enters the key — it decides *how* a
    // matched pair diffs. Postgres can CREATE OR REPLACE across a body change
    // but never across a return-type change (42P13: "cannot change return
    // type of existing function"), so when both sides know their result type
    // (`Some`) and it differs, the pair diffs as Removed + Added (drop, then
    // create). `result_type == None` means the snapshot predates result-type
    // capture (legacy); such a side can never prove a return-type change, so
    // the pair degrades to an in-place CREATE OR REPLACE — never a drop.
    for (name, l) in left {
        let qual = QualifiedName::new(sname, name.as_str());
        match right.get(name) {
            None => out.push(Change::FunctionRemoved {
                qual,
                function: l.clone(),
            }),
            Some(r) => {
                let result_type_differs = matches!(
                    (&l.result_type, &r.result_type),
                    (Some(lt), Some(rt)) if lt != rt
                );
                if result_type_differs {
                    out.push(Change::FunctionRemoved {
                        qual: qual.clone(),
                        function: l.clone(),
                    });
                    out.push(Change::FunctionAdded {
                        qual,
                        function: r.clone(),
                    });
                } else if l.definition != r.definition {
                    out.push(Change::FunctionChanged {
                        qual,
                        before: l.clone(),
                        after: r.clone(),
                    });
                } else if r.acl.is_some() && l.acl != r.acl {
                    // acl == None on the desired side means grants are
                    // unmanaged for this function — never a change.
                    out.push(Change::FunctionGrantsChanged {
                        qual,
                        before: l.clone(),
                        after: r.clone(),
                    });
                }
            }
        }
    }
    for (name, r) in right {
        if !left.contains_key(name) {
            out.push(Change::FunctionAdded {
                qual: QualifiedName::new(sname, name.as_str()),
                function: r.clone(),
            });
        }
    }
}

fn diff_view_bucket(
    sname: &str,
    left: &BTreeMap<String, View>,
    right: &BTreeMap<String, View>,
    materialized: bool,
    out: &mut Vec<Change>,
) {
    for (name, lv) in left {
        let qual = QualifiedName::new(sname, name);
        match right.get(name) {
            None => out.push(Change::ViewRemoved {
                qual,
                materialized,
                depends_on: lv.depends_on.clone(),
            }),
            Some(rv) if lv != rv => out.push(Change::ViewChanged {
                qual,
                materialized,
                before: lv.clone(),
                after: rv.clone(),
            }),
            _ => {}
        }
    }
    for (name, rv) in right {
        if !left.contains_key(name) {
            out.push(Change::ViewAdded {
                qual: QualifiedName::new(sname, name),
                materialized,
                view: rv.clone(),
            });
        }
    }
}

fn diff_sequences(
    sname: &str,
    left: &BTreeMap<String, Sequence>,
    right: &BTreeMap<String, Sequence>,
    out: &mut Vec<Change>,
) {
    for (name, l) in left {
        let qual = QualifiedName::new(sname, name);
        match right.get(name) {
            None => out.push(Change::SequenceRemoved { qual }),
            Some(r) if l != r => out.push(Change::SequenceChanged {
                qual,
                before: l.clone(),
                after: r.clone(),
            }),
            _ => {}
        }
    }
    for (name, r) in right {
        if !left.contains_key(name) {
            out.push(Change::SequenceAdded {
                qual: QualifiedName::new(sname, name),
                sequence: r.clone(),
            });
        }
    }
}

fn diff_types(
    sname: &str,
    left: &BTreeMap<String, UserType>,
    right: &BTreeMap<String, UserType>,
    out: &mut Vec<Change>,
) {
    for (name, l) in left {
        let qual = QualifiedName::new(sname, name);
        match right.get(name) {
            None => out.push(Change::TypeRemoved { qual }),
            Some(r) if l != r => out.push(Change::TypeChanged {
                qual,
                before: l.clone(),
                after: r.clone(),
            }),
            _ => {}
        }
    }
    for (name, r) in right {
        if !left.contains_key(name) {
            out.push(Change::TypeAdded {
                qual: QualifiedName::new(sname, name),
                user_type: r.clone(),
            });
        }
    }
}

fn diff_table(qual: &QualifiedName, left: &Table, right: &Table, out: &mut Vec<Change>) {
    diff_columns(qual, &left.columns, &right.columns, out);
    diff_primary_key(qual, &left.primary_key, &right.primary_key, out);
    diff_named_map(
        &left.indexes,
        &right.indexes,
        |name, idx| Change::IndexAdded {
            table: qual.clone(),
            name: name.clone(),
            index: idx.clone(),
        },
        |name| Change::IndexRemoved {
            table: qual.clone(),
            name: name.clone(),
        },
        |name, before, after| Change::IndexChanged {
            table: qual.clone(),
            name: name.clone(),
            before: before.clone(),
            after: after.clone(),
        },
        out,
    );
    diff_named_map(
        &left.constraints,
        &right.constraints,
        |name, c| Change::ConstraintAdded {
            table: qual.clone(),
            name: name.clone(),
            constraint: c.clone(),
        },
        |name| Change::ConstraintRemoved {
            table: qual.clone(),
            name: name.clone(),
        },
        |name, before, after| Change::ConstraintChanged {
            table: qual.clone(),
            name: name.clone(),
            before: before.clone(),
            after: after.clone(),
        },
        out,
    );
    diff_named_map(
        &left.triggers,
        &right.triggers,
        |name, t| Change::TriggerAdded {
            table: qual.clone(),
            name: name.clone(),
            trigger: t.clone(),
        },
        |name| Change::TriggerRemoved {
            table: qual.clone(),
            name: name.clone(),
        },
        |name, before, after| Change::TriggerChanged {
            table: qual.clone(),
            name: name.clone(),
            before: before.clone(),
            after: after.clone(),
        },
        out,
    );
    // Roles in a PG policy are a set; the catalog already sorts them, but
    // snapshot files (or programmatically-constructed Policy structs in tests)
    // may carry the source ordering. Normalize both sides before comparing
    // so role-order-only differences don't fire a spurious DROP+CREATE.
    let left_policies = normalize_policy_roles(&left.policies);
    let right_policies = normalize_policy_roles(&right.policies);
    diff_named_map(
        &left_policies,
        &right_policies,
        |name, p| Change::PolicyAdded {
            table: qual.clone(),
            name: name.clone(),
            policy: p.clone(),
        },
        |name| Change::PolicyRemoved {
            table: qual.clone(),
            name: name.clone(),
        },
        |name, before, after| Change::PolicyChanged {
            table: qual.clone(),
            name: name.clone(),
            before: before.clone(),
            after: after.clone(),
        },
        out,
    );
    if left.rls_enabled != right.rls_enabled {
        if right.rls_enabled {
            out.push(Change::RlsEnabled {
                table: qual.clone(),
            });
        } else {
            out.push(Change::RlsDisabled {
                table: qual.clone(),
            });
        }
    }
    if left.partition_by != right.partition_by {
        out.push(Change::PartitionByChanged {
            table: qual.clone(),
            before: left.partition_by.clone(),
            after: right.partition_by.clone(),
        });
    }
    if left.partition_of != right.partition_of {
        out.push(Change::PartitionOfChanged {
            table: qual.clone(),
            before: left.partition_of.clone(),
            after: right.partition_of.clone(),
        });
    }
}

fn normalize_policy_roles(policies: &BTreeMap<String, Policy>) -> BTreeMap<String, Policy> {
    policies
        .iter()
        .map(|(name, p)| {
            let mut p = p.clone();
            p.roles = normalize_roles(p.roles);
            (name.clone(), p)
        })
        .collect()
}

fn diff_columns(qual: &QualifiedName, left: &[Column], right: &[Column], out: &mut Vec<Change>) {
    let lcols: BTreeMap<&str, &Column> = left.iter().map(|c| (c.name.as_str(), c)).collect();
    let rcols: BTreeMap<&str, &Column> = right.iter().map(|c| (c.name.as_str(), c)).collect();

    for (name, lcol) in &lcols {
        match rcols.get(name) {
            None => out.push(Change::ColumnRemoved {
                table: qual.clone(),
                name: (*name).to_string(),
            }),
            Some(rcol) if lcol != rcol => out.push(Change::ColumnChanged {
                table: qual.clone(),
                name: (*name).to_string(),
                before: (*lcol).clone(),
                after: (*rcol).clone(),
            }),
            _ => {}
        }
    }
    for (name, rcol) in &rcols {
        if !lcols.contains_key(name) {
            out.push(Change::ColumnAdded {
                table: qual.clone(),
                column: (*rcol).clone(),
            });
        }
    }
}

fn diff_primary_key(
    qual: &QualifiedName,
    left: &Option<Index>,
    right: &Option<Index>,
    out: &mut Vec<Change>,
) {
    match (left, right) {
        (None, None) => {}
        (Some(l), Some(r)) if l == r => {}
        (Some(l), Some(r)) => out.push(Change::PrimaryKeyChanged {
            table: qual.clone(),
            before: l.clone(),
            after: r.clone(),
        }),
        (Some(l), None) => out.push(Change::PrimaryKeyRemoved {
            table: qual.clone(),
            index: l.clone(),
        }),
        (None, Some(r)) => out.push(Change::PrimaryKeyAdded {
            table: qual.clone(),
            index: r.clone(),
        }),
    }
}

fn diff_named_map<V, FAdd, FRm, FCh>(
    left: &BTreeMap<String, V>,
    right: &BTreeMap<String, V>,
    on_add: FAdd,
    on_rm: FRm,
    on_change: FCh,
    out: &mut Vec<Change>,
) where
    V: Clone + PartialEq,
    FAdd: Fn(&String, &V) -> Change,
    FRm: Fn(&String) -> Change,
    FCh: Fn(&String, &V, &V) -> Change,
{
    for (name, lv) in left {
        match right.get(name) {
            None => out.push(on_rm(name)),
            Some(rv) if lv != rv => out.push(on_change(name, lv, rv)),
            _ => {}
        }
    }
    for (name, rv) in right {
        if !left.contains_key(name) {
            out.push(on_add(name, rv));
        }
    }
}
