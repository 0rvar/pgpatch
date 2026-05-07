use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Schema {
    #[serde(default)]
    pub extensions: BTreeMap<String, Extension>,
    #[serde(default)]
    pub schemas: BTreeMap<String, Namespace>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Namespace {
    #[serde(default)]
    pub tables: BTreeMap<String, Table>,
    #[serde(default)]
    pub views: BTreeMap<String, View>,
    #[serde(default)]
    pub materialized_views: BTreeMap<String, View>,
    #[serde(default)]
    pub sequences: BTreeMap<String, Sequence>,
    #[serde(default)]
    pub types: BTreeMap<String, UserType>,
    #[serde(default)]
    pub functions: BTreeMap<String, Function>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Extension {
    pub version: String,
    pub schema: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<Column>,
    #[serde(default)]
    pub primary_key: Option<Index>,
    #[serde(default)]
    pub indexes: BTreeMap<String, Index>,
    #[serde(default)]
    pub constraints: BTreeMap<String, Constraint>,
    #[serde(default)]
    pub triggers: BTreeMap<String, Trigger>,
    #[serde(default)]
    pub policies: BTreeMap<String, Policy>,
    #[serde(default)]
    pub rls_enabled: bool,
    #[serde(default)]
    pub partition_by: Option<PartitionBy>,
    #[serde(default)]
    pub partition_of: Option<PartitionInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub identity: Option<Identity>,
    #[serde(default)]
    pub generated: Option<String>,
    #[serde(default)]
    pub collation: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Identity {
    Always,
    ByDefault,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Index {
    pub definition: String,
    #[serde(default)]
    pub unique: bool,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Constraint {
    pub kind: String,
    pub definition: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Trigger {
    pub definition: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Policy {
    pub command: String,
    pub permissive: bool,
    pub roles: Vec<String>,
    #[serde(default)]
    pub qual: Option<String>,
    #[serde(default)]
    pub with_check: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct View {
    pub definition: String,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    /// Qualified names (`schema.relname`) of relations this view's body
    /// references. Used by the emitter to topo-sort drops/creates so that
    /// `v_a` (which selects from `v_b`) is dropped before `v_b` and created
    /// after it. Empty for tables-only views.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sequence {
    pub data_type: String,
    pub start: i64,
    pub increment: i64,
    pub min_value: i64,
    pub max_value: i64,
    pub cache: i64,
    pub cycle: bool,
    #[serde(default)]
    pub owned_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserType {
    Enum { values: Vec<String> },
    Composite { fields: Vec<(String, String)> },
    Domain { base_type: String, definition: String },
    Range { subtype: String, definition: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Function {
    pub definition: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartitionInfo {
    /// `schema.table` of the parent.
    pub parent: String,
    /// Raw clause from `pg_get_expr(relpartbound, oid)`. Either
    /// `FOR VALUES FROM (...) TO (...)` (RANGE), `FOR VALUES IN (...)` (LIST),
    /// `FOR VALUES WITH (modulus N, remainder M)` (HASH), or `DEFAULT`.
    pub bound: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartitionBy {
    /// `RANGE` | `LIST` | `HASH`.
    pub strategy: String,
    /// Key expression as returned by `pg_get_partkeydef`, with the leading
    /// strategy keyword stripped — e.g. `(started_at)` or `(name)`.
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QualifiedName {
    pub schema: String,
    pub name: String,
}

impl QualifiedName {
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self { schema: schema.into(), name: name.into() }
    }
}

impl std::fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.schema, self.name)
    }
}
