//! Rustls-backed TLS connector for the sync `postgres` crate.
//!
//! Builds a `MakeRustlsConnect` that performs full certificate + hostname
//! validation against Mozilla's webpki root store (matching libpq's
//! `verify-full`). Safe to pass even when the connection string carries
//! `sslmode=disable`: tokio-postgres' libpq-style negotiation skips the
//! handshake in that case, so a single connector covers all sslmode
//! variants from `disable` through `verify-full`.
//!
//! Self-signed certs and `verify-ca` semantics aren't supported — both
//! would need a custom `ServerCertVerifier`. If a managed Postgres provider
//! presents a cert chain rooted in a public CA (Supabase, RDS, GCP), the
//! default config Just Works.

use tokio_postgres_rustls::MakeRustlsConnect;

pub fn connector() -> MakeRustlsConnect {
    // rustls 0.23 requires a process-wide default crypto provider to be
    // installed before any ClientConfig is built. Subsequent installs
    // return Err — ignored, since "already installed" is fine.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    MakeRustlsConnect::new(config)
}
