//! Rustls-backed TLS for the sync `postgres` crate, with libpq-style
//! `sslmode` handling.
//!
//! tokio-postgres natively understands only `disable`, `allow`, `prefer`, and
//! `require` — its parser rejects `verify-ca` and `verify-full`. Everything
//! stricter than `require` is a connector-level concern in tokio-postgres
//! anyway, so we sniff the sslmode out of the connection string ourselves,
//! rewrite the URL down to a tokio-compatible mode, then build a rustls
//! `ClientConfig` matching the user's intent.
//!
//! The mapping mirrors libpq:
//!
//! - `disable` — plaintext only; the connector is never invoked.
//! - `prefer` (default) — TLS with full validation if available; falls back
//!   to plaintext otherwise.
//! - `require` — TLS required, *no certificate validation*. Lets you talk
//!   to managed Postgres with self-signed leaf certs (Supabase) without
//!   shipping the CA bundle.
//! - `verify-ca` — TLS with chain validation. Treated like `verify-full`
//!   here; rustls always performs hostname matching, so getting true
//!   chain-only semantics would need a custom `ServerCertVerifier`.
//! - `verify-full` — TLS with chain + hostname validation. Default for any
//!   connection string that doesn't say otherwise (when TLS happens at all).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::{
    DigitallySignedStruct, Error, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use tokio_postgres_rustls::MakeRustlsConnect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

#[derive(Debug)]
pub struct ConnectionSpec {
    /// Connection string with `verify-ca`/`verify-full` rewritten to
    /// `require` and `sslrootcert=` stripped, so tokio-postgres' parser
    /// accepts it.
    pub url: String,
    pub sslmode: SslMode,
    /// Path to a PEM-encoded CA bundle, if the user supplied
    /// `sslrootcert=...`. When present, this replaces the public webpki
    /// trust store. Used for managed Postgres providers that present
    /// certs under a private CA (Supabase, Aiven, self-hosted).
    pub root_cert: Option<PathBuf>,
}

pub fn parse(connection: &str) -> ConnectionSpec {
    // sslmode and sslrootcert show up as `key=value` pairs in both URL
    // (`?key=v&key2=v2`) and libpq-keyvalue (`key=v key2=v2`) syntaxes.
    // Splitting on the union of separators handles both without a real
    // URL parser.
    let sslmode = extract_kv(connection, "sslmode")
        .map(|m| match m.to_ascii_lowercase().as_str() {
            "disable" => SslMode::Disable,
            "allow" | "prefer" => SslMode::Prefer,
            "require" => SslMode::Require,
            "verify-ca" => SslMode::VerifyCa,
            "verify-full" => SslMode::VerifyFull,
            _ => SslMode::Prefer,
        })
        .unwrap_or(SslMode::Prefer);

    let root_cert = extract_kv(connection, "sslrootcert").map(PathBuf::from);

    let url = rewrite_sslmode(connection, "verify-full", "require");
    let url = rewrite_sslmode(&url, "verify-ca", "require");
    let url = strip_param(&url, "sslrootcert");
    ConnectionSpec { url, sslmode, root_cert }
}

fn extract_kv<'a>(connection: &'a str, key: &str) -> Option<&'a str> {
    connection
        .split(|c: char| matches!(c, '?' | '&' | ' ' | ';'))
        .find_map(|kv| {
            let kv = kv.trim();
            let (k, v) = kv.split_once('=')?;
            if k.eq_ignore_ascii_case(key) { Some(v) } else { None }
        })
}

fn is_sep(c: char) -> bool {
    matches!(c, '?' | '&' | ' ' | ';' | '\t' | '\n')
}

/// Remove `key=value` pairs (across the same separator alphabet) from the
/// connection string, including the separator that introduced them so the
/// remaining string stays well-formed (no dangling `&`, no double spaces).
/// A leading `?` is preserved so the URL still has its query-string marker.
fn strip_param(s: &str, key: &str) -> String {
    let needle = format!("{key}=");
    let needle_lower = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let lower = s.to_ascii_lowercase();
    let mut i = 0;
    while let Some(rel) = lower[i..].find(&needle_lower) {
        let start = i + rel;
        // Anchor: make sure the match begins a real key=value pair, not a
        // suffix of some other key (`nosslrootcert=` must not match).
        let prev = if start == 0 { None } else { s[..start].chars().next_back() };
        let anchored = matches!(prev, None) || prev.map(is_sep).unwrap_or(false);
        if !anchored {
            out.push_str(&s[i..start + needle.len()]);
            i = start + needle.len();
            continue;
        }
        let end = s[start + needle.len()..]
            .find(is_sep)
            .map(|p| start + needle.len() + p)
            .unwrap_or(s.len());

        // Decide what to cut. We want to consume the entry plus its
        // *introducing* separator so we don't leave a `& ` or trailing
        // space behind. The first `?` in a URL is the query-string marker
        // — keep it if the entry we're stripping is the only param, so
        // the URL remains valid.
        let mut cut_start = start;
        match prev {
            Some('?') => {
                // Only consume the `?` if there's still content after the
                // stripped param (a `?` ahead of nothing is harmless and
                // keeps URL shape, but `?&` would be malformed).
                if s[end..].starts_with('&') {
                    cut_start -= 1;
                }
            }
            Some('&' | ' ' | ';' | '\t' | '\n') => {
                cut_start -= 1;
            }
            _ => {}
        }
        out.push_str(&s[i..cut_start]);
        i = end;
    }
    out.push_str(&s[i..]);
    out
}

fn rewrite_sslmode(s: &str, from: &str, to: &str) -> String {
    let needle = "sslmode=";
    let mut out = String::with_capacity(s.len());
    let lower = s.to_ascii_lowercase();
    let mut i = 0;
    while let Some(rel) = lower[i..].find(needle) {
        let start = i + rel + needle.len();
        out.push_str(&s[i..start]);
        let end = s[start..]
            .find(|c: char| matches!(c, '?' | '&' | ' ' | ';' | '\t' | '\n'))
            .map(|p| start + p)
            .unwrap_or(s.len());
        let value = s[start..end].to_ascii_lowercase();
        if value == from {
            out.push_str(to);
        } else {
            out.push_str(&s[start..end]);
        }
        i = end;
    }
    out.push_str(&s[i..]);
    out
}

pub fn connector(spec: &ConnectionSpec) -> Result<MakeRustlsConnect> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Escape hatch for managed Postgres with self-signed/private-CA certs
    // when shipping the CA bundle alongside the binary isn't practical.
    // Takes priority over whatever the user wrote in the URL, since the
    // connection string is often generated by tooling out of your control.
    let insecure = matches!(
        std::env::var("PGPATCH_TLS_INSECURE").as_deref(),
        Ok("1" | "true" | "yes")
    );

    let use_no_verify = insecure || spec.sslmode == SslMode::Require;

    let config = if use_no_verify {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth()
    } else {
        let roots = match &spec.root_cert {
            Some(path) => load_root_store(path)?,
            None => rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            },
        };
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    };

    Ok(MakeRustlsConnect::new(config))
}

fn load_root_store(path: &std::path::Path) -> Result<rustls::RootCertStore> {
    let pem = std::fs::read(path)
        .with_context(|| format!("reading sslrootcert from {}", path.display()))?;
    let mut reader = std::io::Cursor::new(pem);
    let mut store = rustls::RootCertStore::empty();
    let mut added = 0;
    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.with_context(|| format!("parsing PEM in {}", path.display()))?;
        store
            .add(cert)
            .with_context(|| format!("adding cert from {}", path.display()))?;
        added += 1;
    }
    if added == 0 {
        anyhow::bail!("no certificates found in {}", path.display());
    }
    Ok(store)
}

/// Accept-anything verifier used for `sslmode=require`. The transport is
/// still encrypted, but a MITM with any cert succeeds. That's the deliberate
/// libpq trade-off for `require`: confidentiality, no authentication.
#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_styles() {
        assert_eq!(parse("postgres://u:p@h/db").sslmode, SslMode::Prefer);
        assert_eq!(parse("postgres://u:p@h/db?sslmode=disable").sslmode, SslMode::Disable);
        assert_eq!(parse("postgres://u:p@h/db?sslmode=require").sslmode, SslMode::Require);
        assert_eq!(
            parse("postgres://u:p@h/db?sslmode=verify-full").sslmode,
            SslMode::VerifyFull,
        );
        assert_eq!(
            parse("postgres://u:p@h/db?sslmode=verify-ca&application_name=x").sslmode,
            SslMode::VerifyCa,
        );
    }

    #[test]
    fn parse_keyvalue_style() {
        let c = parse("host=h user=u sslmode=require dbname=d");
        assert_eq!(c.sslmode, SslMode::Require);
        assert_eq!(c.url, "host=h user=u sslmode=require dbname=d");
        assert!(c.root_cert.is_none());
    }

    #[test]
    fn sslrootcert_extracted_and_stripped_from_url() {
        let c = parse(
            "postgres://u@h/db?sslmode=verify-full&sslrootcert=/etc/supabase-ca.crt",
        );
        assert_eq!(c.sslmode, SslMode::VerifyFull);
        assert_eq!(c.root_cert.as_deref(), Some(std::path::Path::new("/etc/supabase-ca.crt")));
        // tokio-postgres doesn't recognize sslrootcert; it must be gone.
        assert_eq!(c.url, "postgres://u@h/db?sslmode=require");
    }

    #[test]
    fn sslrootcert_keyvalue_style() {
        let c = parse("host=h sslmode=verify-full sslrootcert=/tmp/ca.pem");
        assert_eq!(c.root_cert.as_deref(), Some(std::path::Path::new("/tmp/ca.pem")));
        assert_eq!(c.url, "host=h sslmode=require");
    }

    #[test]
    fn strip_param_does_not_match_substring_keys() {
        // A key whose name *contains* `sslrootcert` as a suffix shouldn't be
        // stripped — anchor on the param boundary, not raw substring.
        let c = parse("postgres://h/db?nosslrootcert=keep&sslmode=require");
        assert_eq!(c.root_cert, None);
        assert!(c.url.contains("nosslrootcert=keep"));
    }

    #[test]
    fn verify_full_is_rewritten_to_require_for_tokio_postgres() {
        let c = parse("postgres://u@h/db?sslmode=verify-full");
        assert_eq!(c.sslmode, SslMode::VerifyFull);
        assert_eq!(c.url, "postgres://u@h/db?sslmode=require");
    }

    #[test]
    fn verify_ca_is_rewritten_to_require() {
        let c = parse("host=h sslmode=verify-ca");
        assert_eq!(c.sslmode, SslMode::VerifyCa);
        assert_eq!(c.url, "host=h sslmode=require");
    }

    #[test]
    fn rewrite_preserves_other_params() {
        let c = parse("postgres://u@h/db?sslmode=verify-full&application_name=pgpatch");
        assert_eq!(
            c.url,
            "postgres://u@h/db?sslmode=require&application_name=pgpatch"
        );
    }

    #[test]
    fn unknown_sslmode_falls_back_to_prefer() {
        // Forward compat: a future libpq sslmode shouldn't crash us. Treat
        // unknown values as `prefer`, which is the libpq default.
        assert_eq!(parse("host=h sslmode=zomgwhat").sslmode, SslMode::Prefer);
    }
}
