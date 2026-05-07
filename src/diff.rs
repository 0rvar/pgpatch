use crate::model::{
    Column, Constraint, Extension, Function, Index, Namespace, PartitionBy, PartitionInfo, Policy,
    QualifiedName, Schema, Sequence, Table, Trigger, UserType, View,
};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    SchemaAdded { name: String },
    SchemaRemoved { name: String },

    TableAdded { qual: QualifiedName, table: Table },
    TableRemoved { qual: QualifiedName },

    ColumnAdded { table: QualifiedName, column: Column },
    ColumnRemoved { table: QualifiedName, name: String },
    ColumnChanged { table: QualifiedName, name: String, before: Column, after: Column },

    PrimaryKeyAdded { table: QualifiedName, index: Index },
    PrimaryKeyRemoved { table: QualifiedName, index: Index },
    PrimaryKeyChanged { table: QualifiedName, before: Index, after: Index },

    IndexAdded { table: QualifiedName, name: String, index: Index },
    IndexRemoved { table: QualifiedName, name: String },
    IndexChanged { table: QualifiedName, name: String, before: Index, after: Index },

    ConstraintAdded { table: QualifiedName, name: String, constraint: Constraint },
    ConstraintRemoved { table: QualifiedName, name: String },
    ConstraintChanged { table: QualifiedName, name: String, before: Constraint, after: Constraint },

    RlsEnabled { table: QualifiedName },
    RlsDisabled { table: QualifiedName },

    ViewAdded { qual: QualifiedName, materialized: bool, view: View },
    ViewRemoved { qual: QualifiedName, materialized: bool, depends_on: Vec<String> },
    ViewChanged { qual: QualifiedName, materialized: bool, before: View, after: View },

    SequenceAdded { qual: QualifiedName, sequence: Sequence },
    SequenceRemoved { qual: QualifiedName },
    SequenceChanged { qual: QualifiedName, before: Sequence, after: Sequence },

    TypeAdded { qual: QualifiedName, user_type: UserType },
    TypeRemoved { qual: QualifiedName },
    TypeChanged { qual: QualifiedName, before: UserType, after: UserType },

    TriggerAdded { table: QualifiedName, name: String, trigger: Trigger },
    TriggerRemoved { table: QualifiedName, name: String },
    TriggerChanged { table: QualifiedName, name: String, before: Trigger, after: Trigger },

    PolicyAdded { table: QualifiedName, name: String, policy: Policy },
    PolicyRemoved { table: QualifiedName, name: String },
    PolicyChanged { table: QualifiedName, name: String, before: Policy, after: Policy },

    FunctionAdded { qual: QualifiedName, function: Function },
    FunctionRemoved { qual: QualifiedName },
    FunctionChanged { qual: QualifiedName, before: Function, after: Function },

    ExtensionAdded { name: String, extension: Extension },
    ExtensionRemoved { name: String },
    ExtensionChanged { name: String, before: Extension, after: Extension },

    PartitionByChanged { table: QualifiedName, before: Option<PartitionBy>, after: Option<PartitionBy> },
    PartitionOfChanged { table: QualifiedName, before: Option<PartitionInfo>, after: Option<PartitionInfo> },
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
        let Some(rns) = right.schemas.get(sname) else { continue };
        diff_namespace(sname, lns, rns, &mut out);
    }

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
    diff_view_bucket(sname, &left.materialized_views, &right.materialized_views, true, out);
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
    for (name, l) in left {
        let qual = QualifiedName::new(sname, name);
        match right.get(name) {
            None => out.push(Change::FunctionRemoved { qual }),
            Some(r) if l != r => out.push(Change::FunctionChanged {
                qual,
                before: l.clone(),
                after: r.clone(),
            }),
            _ => {}
        }
    }
    for (name, r) in right {
        if !left.contains_key(name) {
            out.push(Change::FunctionAdded {
                qual: QualifiedName::new(sname, name),
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
        |name, idx| Change::IndexAdded { table: qual.clone(), name: name.clone(), index: idx.clone() },
        |name| Change::IndexRemoved { table: qual.clone(), name: name.clone() },
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
        |name| Change::ConstraintRemoved { table: qual.clone(), name: name.clone() },
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
        |name, t| Change::TriggerAdded { table: qual.clone(), name: name.clone(), trigger: t.clone() },
        |name| Change::TriggerRemoved { table: qual.clone(), name: name.clone() },
        |name, before, after| Change::TriggerChanged {
            table: qual.clone(),
            name: name.clone(),
            before: before.clone(),
            after: after.clone(),
        },
        out,
    );
    diff_named_map(
        &left.policies,
        &right.policies,
        |name, p| Change::PolicyAdded { table: qual.clone(), name: name.clone(), policy: p.clone() },
        |name| Change::PolicyRemoved { table: qual.clone(), name: name.clone() },
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
            out.push(Change::RlsEnabled { table: qual.clone() });
        } else {
            out.push(Change::RlsDisabled { table: qual.clone() });
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
