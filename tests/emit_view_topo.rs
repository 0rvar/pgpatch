use pgpatch::diff::Change;
use pgpatch::emit::sql;
use pgpatch::model::{QualifiedName, View};

fn qn(s: &str, n: &str) -> QualifiedName {
    QualifiedName::new(s, n)
}

fn view_with_deps(definition: &str, deps: &[&str]) -> View {
    View {
        definition: definition.into(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn position_of(haystack: &str, needle: &str) -> usize {
    haystack.find(needle).unwrap_or_else(|| panic!("missing: {needle}\nin:\n{haystack}"))
}

#[test]
fn create_views_emit_in_dependency_order() {
    // v_top SELECTs from v_mid SELECTs from v_base. Alphabetical order
    // would be base, mid, top — which is also the correct topo order.
    let out = sql(&[
        Change::ViewAdded { qual: qn("public", "v_top"), materialized: false, view: view_with_deps("SELECT * FROM public.v_mid;", &["public.v_mid"]) },
        Change::ViewAdded { qual: qn("public", "v_mid"), materialized: false, view: view_with_deps("SELECT * FROM public.v_base;", &["public.v_base"]) },
        Change::ViewAdded { qual: qn("public", "v_base"), materialized: false, view: view_with_deps("SELECT 1 AS a;", &[]) },
    ]);
    let base_pos = position_of(&out, "CREATE VIEW public.v_base");
    let mid_pos = position_of(&out, "CREATE VIEW public.v_mid");
    let top_pos = position_of(&out, "CREATE VIEW public.v_top");
    assert!(base_pos < mid_pos, "v_base must come before v_mid; got:\n{out}");
    assert!(mid_pos < top_pos, "v_mid must come before v_top; got:\n{out}");
}

#[test]
fn create_views_topo_order_overrides_alphabetical_when_needed() {
    // Alphabetical input order would be wrong: a depends on z, but 'a' < 'z'.
    let out = sql(&[
        Change::ViewAdded { qual: qn("public", "a_top"), materialized: false, view: view_with_deps("SELECT * FROM public.z_base;", &["public.z_base"]) },
        Change::ViewAdded { qual: qn("public", "z_base"), materialized: false, view: view_with_deps("SELECT 1;", &[]) },
    ]);
    let z_pos = position_of(&out, "CREATE VIEW public.z_base");
    let a_pos = position_of(&out, "CREATE VIEW public.a_top");
    assert!(z_pos < a_pos, "z_base must come before a_top; got:\n{out}");
}

#[test]
fn dropped_views_emit_in_reverse_dependency_order() {
    // v_top depends on v_base. Drop must hit v_top *before* v_base, otherwise
    // PG refuses with "cannot drop view v_base because other objects depend on it".
    let out = sql(&[
        Change::ViewRemoved {
            qual: qn("public", "v_base"),
            materialized: false,
            depends_on: vec![],
        },
        Change::ViewRemoved {
            qual: qn("public", "v_top"),
            materialized: false,
            depends_on: vec!["public.v_base".into()],
        },
    ]);
    let top_pos = position_of(&out, "DROP VIEW public.v_top;");
    let base_pos = position_of(&out, "DROP VIEW public.v_base;");
    assert!(top_pos < base_pos, "drop must go top→base; got:\n{out}");
}

#[test]
fn changed_views_drop_in_reverse_topo_then_create_in_topo() {
    // Both views change. Drops: v_top first then v_base. Creates: v_base first
    // then v_top.
    let out = sql(&[
        Change::ViewChanged {
            qual: qn("public", "v_base"),
            materialized: false,
            before: view_with_deps("SELECT 1;", &[]),
            after: view_with_deps("SELECT 2;", &[]),
        },
        Change::ViewChanged {
            qual: qn("public", "v_top"),
            materialized: false,
            before: view_with_deps("SELECT * FROM public.v_base;", &["public.v_base"]),
            after: view_with_deps("SELECT * FROM public.v_base WHERE true;", &["public.v_base"]),
        },
    ]);

    let drop_top = position_of(&out, "DROP VIEW public.v_top");
    let drop_base = position_of(&out, "DROP VIEW public.v_base");
    let create_base = position_of(&out, "CREATE VIEW public.v_base");
    let create_top = position_of(&out, "CREATE VIEW public.v_top");

    assert!(drop_top < drop_base, "v_top must drop before v_base; got:\n{out}");
    assert!(drop_base < create_base, "all drops before any creates; got:\n{out}");
    assert!(create_base < create_top, "v_base must create before v_top; got:\n{out}");
}

#[test]
fn views_with_no_mutual_deps_keep_input_order() {
    // Two unrelated views — neither depends on the other. The sort must be
    // stable and preserve input order so emit output is deterministic.
    let out = sql(&[
        Change::ViewAdded { qual: qn("public", "v_alpha"), materialized: false, view: view_with_deps("SELECT 1;", &["public.t_x"]) },
        Change::ViewAdded { qual: qn("public", "v_beta"),  materialized: false, view: view_with_deps("SELECT 2;", &["public.t_y"]) },
    ]);
    let alpha_pos = position_of(&out, "CREATE VIEW public.v_alpha");
    let beta_pos = position_of(&out, "CREATE VIEW public.v_beta");
    assert!(alpha_pos < beta_pos, "input order not preserved; got:\n{out}");
}

#[test]
fn dependency_to_object_outside_batch_is_ignored() {
    // v_top references public.t_external which is not part of this diff. The
    // sorter must not fail or block on it — only intra-batch edges matter.
    let out = sql(&[
        Change::ViewAdded { qual: qn("public", "v_top"), materialized: false, view: view_with_deps("SELECT * FROM public.t_external;", &["public.t_external"]) },
    ]);
    assert!(out.contains("CREATE VIEW public.v_top"), "got:\n{out}");
}

#[test]
fn topo_sort_tolerates_cycle_without_panicking() {
    // Postgres rejects mutual view recursion, but if the input artefact
    // contains a cycle (perhaps from corruption or manual editing), the
    // emitter should fall back to input order rather than crash.
    let out = sql(&[
        Change::ViewAdded { qual: qn("public", "a"), materialized: false, view: view_with_deps("SELECT * FROM public.b;", &["public.b"]) },
        Change::ViewAdded { qual: qn("public", "b"), materialized: false, view: view_with_deps("SELECT * FROM public.a;", &["public.a"]) },
    ]);
    assert!(out.contains("CREATE VIEW public.a"), "got:\n{out}");
    assert!(out.contains("CREATE VIEW public.b"), "got:\n{out}");
}

#[test]
fn materialized_views_share_the_same_topo_bucket_as_views() {
    // A materialized view that selects from a normal view must still respect
    // the dependency: the upstream non-mat view exists before the mat view.
    let out = sql(&[
        Change::ViewAdded {
            qual: qn("public", "mv_top"),
            materialized: true,
            view: view_with_deps("SELECT * FROM public.v_base;", &["public.v_base"]),
        },
        Change::ViewAdded {
            qual: qn("public", "v_base"),
            materialized: false,
            view: view_with_deps("SELECT 1;", &[]),
        },
    ]);
    let base_pos = position_of(&out, "CREATE VIEW public.v_base");
    let mv_pos = position_of(&out, "CREATE MATERIALIZED VIEW public.mv_top");
    assert!(base_pos < mv_pos, "v_base must come before mv_top; got:\n{out}");
}
