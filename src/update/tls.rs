use anyhow::{Context, Result, ensure};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use ureq::tls::{Certificate, PemItem, RootCerts, TlsConfig};

const MAX_CA_BUNDLE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn config_from_env() -> Result<TlsConfig> {
    let ca_file = std::env::var_os("SSL_CERT_FILE");
    config_from_ca_file(ca_file.as_deref().map(Path::new))
}

pub(super) fn config_from_ca_file(path: Option<&Path>) -> Result<TlsConfig> {
    let Some(path) = path else {
        return Ok(TlsConfig::default());
    };
    ensure!(
        !path.as_os_str().is_empty(),
        "SSL_CERT_FILE must not be empty"
    );
    // Do not include paths or bundle contents in errors: host-managed trust
    // material can live alongside secrets in a temporary egress directory.
    let file = File::open(path).context("cannot open SSL_CERT_FILE CA bundle")?;
    let mut pem = Vec::new();
    file.take(MAX_CA_BUNDLE_BYTES + 1)
        .read_to_end(&mut pem)
        .context("cannot read SSL_CERT_FILE CA bundle")?;
    ensure!(
        pem.len() as u64 <= MAX_CA_BUNDLE_BYTES,
        "SSL_CERT_FILE CA bundle exceeds 16 MiB"
    );

    let mut extra_certs = Vec::new();
    let mut validation_store = rustls::RootCertStore::empty();
    for item in ureq::tls::parse_pem(&pem) {
        // The PEM parser can include offending input bytes in its error.
        let item = item.map_err(|_| anyhow::anyhow!("cannot parse SSL_CERT_FILE PEM CA bundle"))?;
        if let PemItem::Certificate(cert) = item {
            // ureq silently ignores invalid DER roots. Validate eagerly so a
            // broken configured bundle cannot silently change the trust set.
            validation_store
                .add(rustls::pki_types::CertificateDer::from(cert.der()))
                .context("SSL_CERT_FILE contains an invalid X.509 certificate")?;
            extra_certs.push(cert);
        }
    }
    ensure!(
        !extra_certs.is_empty(),
        "SSL_CERT_FILE CA bundle contains no PEM certificates"
    );

    // RootCerts::Specific replaces ureq's default roots. Retain Mozilla's
    // public roots as well as the host-provided proxy CA for every endpoint.
    let mut certs: Vec<_> = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|cert| Certificate::from_der(cert.as_ref()).to_owned())
        .collect();
    certs.extend(extra_certs);
    Ok(TlsConfig::builder()
        .root_certs(RootCerts::new_with_certs(&certs))
        .build())
}

#[cfg(test)]
mod tests;
