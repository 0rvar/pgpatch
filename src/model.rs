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
    /// `pg_get_function_result(oid)` — the full RETURNS clause body (e.g.
    /// `integer`, `TABLE(id bigint)`). `Some("")` for procedures. Part of
    /// the diff identity when known: Postgres cannot CREATE OR REPLACE
    /// across a return-type change (42P13), so a result-type difference
    /// must become a drop + create rather than an in-place change. `None`
    /// means the snapshot predates result-type capture (legacy) — a legacy
    /// side can never prove a return-type change, so the diff degrades to
    /// in-place CREATE OR REPLACE instead of drop + create.
    #[serde(default)]
    pub result_type: Option<String>,
    /// Raw `pg_proc.proname`, unquoted. Together with `identity_args` this
    /// lets the emitter build a correctly-quoted executable identity for
    /// DROP/GRANT/REVOKE. Empty in legacy snapshots — the emitter then
    /// falls back to the verbatim map key.
    #[serde(default)]
    pub name: String,
    /// `pg_get_function_identity_arguments(oid)` output, verbatim. May be
    /// empty for zero-argument functions; legacy detection keys off `name`.
    #[serde(default)]
    pub identity_args: String,
    /// Exploded `pg_proc.proacl`, sorted for deterministic equality.
    /// `None` means the ACL column is NULL (implicit default privileges) —
    /// pgpatch treats that as "unmanaged" and never emits grant SQL for it.
    /// `Some(vec![])` is a real, fully-revoked ACL.
    #[serde(default)]
    pub acl: Option<Vec<GrantEntry>>,
}

/// One entry from `aclexplode(pg_proc.proacl)`. `grantee` is `Some(role)`
/// for a real role, or `None` for the PUBLIC pseudo-role (grantee oid 0) —
/// identified by oid, never by spelling, so a real role named "Public" or
/// "PUBLIC" is never confused with the keyword.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct GrantEntry {
    #[serde(default)]
    pub grantee: Option<String>,
    pub privilege: String,
    #[serde(default)]
    pub grantable: bool,
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
