use std::sync::Once;

use anyhow::{anyhow, Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;

/// Generate an in-memory self-signed cert+key for `localhost` and
/// build a rustls [`ServerConfig`] from them. Mirrors the legacy
/// Flask `ssl_context="adhoc"` flag.
#[allow(dead_code)] // wired up in T1B.5
pub fn build_self_signed_config() -> Result<ServerConfig> {
    install_default_crypto_provider();

    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .context("generating self-signed cert")?;
    let cert_pem = cert.cert.pem();
    let key_pem = cert.key_pair.serialize_pem();

    let cert_der = parse_first_cert(&cert_pem)?;
    let key_der = parse_private_key(&key_pem)?;

    ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .context("building rustls ServerConfig from self-signed pair")
}

fn parse_first_cert(pem: &str) -> Result<CertificateDer<'static>> {
    let mut reader = std::io::Cursor::new(pem);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<_, _>>()
        .context("parsing self-signed cert PEM")?;
    certs
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no certificates in self-signed PEM"))
}

fn parse_private_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    let mut reader = std::io::Cursor::new(pem);
    rustls_pemfile::private_key(&mut reader)
        .context("parsing self-signed key PEM")?
        .ok_or_else(|| anyhow!("no private key in self-signed PEM"))
}

#[allow(dead_code)] // called by build_self_signed_config; wired up in T1B.5
fn install_default_crypto_provider() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_self_signed_config_succeeds() {
        let result = build_self_signed_config();
        assert!(
            result.is_ok(),
            "expected self-signed ServerConfig to build, got {:?}",
            result.err()
        );
    }

    #[test]
    fn rcgen_output_round_trips_as_pem_chain() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("rcgen self-signed");
        let cert_pem = cert.cert.pem();
        let key_pem = cert.key_pair.serialize_pem();

        let cert_der = parse_first_cert(&cert_pem).expect("cert parses back");
        // CertificateDer must be non-empty bytes — anything else means
        // the PEM round-trip silently lost the body.
        assert!(!cert_der.as_ref().is_empty());

        let key_der = parse_private_key(&key_pem).expect("key parses back");
        match key_der {
            PrivateKeyDer::Pkcs1(k) => assert!(!k.secret_pkcs1_der().is_empty()),
            PrivateKeyDer::Pkcs8(k) => assert!(!k.secret_pkcs8_der().is_empty()),
            PrivateKeyDer::Sec1(k) => assert!(!k.secret_sec1_der().is_empty()),
            _ => panic!("unexpected key der variant"),
        }
    }
}
