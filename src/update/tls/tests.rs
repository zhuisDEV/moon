use super::config_from_ca_file;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, date_time_ymd,
};
use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::{CertificateError, ServerConfig, ServerConnection, StreamOwned};
use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use ureq::tls::{RootCerts, TlsConfig};

struct Authority {
    certificate: Certificate,
    issuer: Issuer<'static, KeyPair>,
}

impl Authority {
    fn new() -> Self {
        static NEXT_CA: AtomicUsize = AtomicUsize::new(1);
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(
            DnType::CommonName,
            format!("Moon test CA {}", NEXT_CA.fetch_add(1, Ordering::Relaxed)),
        );
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let key = KeyPair::generate().unwrap();
        let certificate = params.self_signed(&key).unwrap();
        Self {
            certificate,
            issuer: Issuer::new(params, key),
        }
    }

    fn server_config(&self, hostname: &str, expired: bool) -> Arc<ServerConfig> {
        let mut params = CertificateParams::new(vec![hostname.to_owned()]).unwrap();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        if expired {
            params.not_before = date_time_ymd(2000, 1, 1);
            params.not_after = date_time_ymd(2001, 1, 1);
        }
        let key = KeyPair::generate().unwrap();
        let certificate = params.signed_by(&key, &self.issuer).unwrap();
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![certificate.der().clone()],
                    PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
                )
                .unwrap();
        Arc::new(config)
    }

    fn write_bundle(&self, path: &Path) {
        fs::write(path, self.certificate.pem()).unwrap();
    }
}

// This uses the production TLS configuration with an entirely local HTTP client.
// Explicitly removing the proxy prevents ambient egress settings from affecting
// these tests, without mutating process-wide environment variables.
fn local_request(tls: TlsConfig, server_config: Arc<ServerConfig>) -> Result<String, ureq::Error> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || -> io::Result<()> {
        // Bound accept as well as reads, so a client-side setup failure cannot
        // leave a test thread blocked indefinitely.
        let deadline = Instant::now() + Duration::from_secs(6);
        let socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error),
            }
        };
        socket.set_nonblocking(false)?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        socket.set_write_timeout(Some(Duration::from_secs(3)))?;
        let connection = ServerConnection::new(server_config).unwrap();
        let mut stream = StreamOwned::new(connection, socket);
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte)?;
            request.push(byte[0]);
            if request.len() > 8 * 1024 {
                return Err(io::ErrorKind::InvalidData.into());
            }
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")?;
        stream.flush()
    });
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .proxy(None)
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();
    let result = agent
        .get(format!("https://localhost:{port}/release-manifest.json"))
        .call()
        .and_then(|mut response| response.body_mut().read_to_string());
    // Certificate-rejection cases intentionally cause the server handshake to
    // fail. The client error below is what determines the reason for rejection.
    let _ = server.join().expect("TLS fixture thread must not panic");
    result
}

fn certificate_error(error: &ureq::Error) -> &CertificateError {
    let rustls_error = match error {
        ureq::Error::Rustls(error) => Some(error),
        ureq::Error::Io(error) => error
            .get_ref()
            .and_then(|error| error.downcast_ref::<rustls::Error>()),
        _ => None,
    };
    match rustls_error {
        Some(rustls::Error::InvalidCertificate(error)) => error,
        _ => panic!("expected certificate verification failure, got {error:?}"),
    }
}

#[test]
fn configured_ca_accepts_its_server_certificate() {
    let temp = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let bundle = temp.path().join("proxy-ca.pem");
    authority.write_bundle(&bundle);
    let tls = config_from_ca_file(Some(&bundle)).unwrap();
    assert!(!tls.disable_verification());
    assert_eq!(
        local_request(tls, authority.server_config("localhost", false)).unwrap(),
        "ok"
    );
}

#[test]
fn default_trust_rejects_an_unconfigured_ca() {
    let authority = Authority::new();
    let tls = config_from_ca_file(None).unwrap();
    let error = local_request(tls, authority.server_config("localhost", false)).unwrap_err();
    assert!(matches!(
        certificate_error(&error),
        CertificateError::UnknownIssuer
    ));
}

#[test]
fn configured_ca_does_not_trust_an_unrelated_issuer() {
    let temp = tempfile::tempdir().unwrap();
    let trusted = Authority::new();
    let untrusted = Authority::new();
    let bundle = temp.path().join("proxy-ca.pem");
    trusted.write_bundle(&bundle);
    let tls = config_from_ca_file(Some(&bundle)).unwrap();
    let error = local_request(tls, untrusted.server_config("localhost", false)).unwrap_err();
    assert!(matches!(
        certificate_error(&error),
        CertificateError::UnknownIssuer
    ));
}

#[test]
fn configured_ca_does_not_disable_hostname_verification() {
    let temp = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let bundle = temp.path().join("proxy-ca.pem");
    authority.write_bundle(&bundle);
    let tls = config_from_ca_file(Some(&bundle)).unwrap();
    let error = local_request(tls, authority.server_config("wrong.invalid", false)).unwrap_err();
    assert!(matches!(
        certificate_error(&error),
        CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }
    ));
}

#[test]
fn configured_ca_does_not_disable_expiry_verification() {
    let temp = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let bundle = temp.path().join("proxy-ca.pem");
    authority.write_bundle(&bundle);
    let tls = config_from_ca_file(Some(&bundle)).unwrap();
    let error = local_request(tls, authority.server_config("localhost", true)).unwrap_err();
    assert!(matches!(
        certificate_error(&error),
        CertificateError::Expired | CertificateError::ExpiredContext { .. }
    ));
}

#[test]
fn bundle_loads_every_ca_and_preserves_mozilla_roots() {
    let temp = tempfile::tempdir().unwrap();
    let first = Authority::new();
    let second = Authority::new();
    let bundle = temp.path().join("proxy-ca.pem");
    fs::write(
        &bundle,
        format!("{}{}", first.certificate.pem(), second.certificate.pem()),
    )
    .unwrap();
    let tls = config_from_ca_file(Some(&bundle)).unwrap();
    let RootCerts::Specific(roots) = tls.root_certs() else {
        panic!("configured CA bundle should use explicit roots");
    };
    for public_root in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        assert!(
            roots.iter().any(|root| root.der() == public_root.as_ref()),
            "configured CA must not remove a bundled Mozilla root"
        );
    }
    for authority in [&first, &second] {
        assert_eq!(
            local_request(tls.clone(), authority.server_config("localhost", false)).unwrap(),
            "ok"
        );
    }
}

#[test]
fn missing_or_unreadable_ca_bundle_fails_before_network() {
    let temp = tempfile::tempdir().unwrap();
    for path in [temp.path().join("missing.pem"), temp.path().to_path_buf()] {
        let error = config_from_ca_file(Some(&path)).unwrap_err();
        assert!(format!("{error:#}").contains("SSL_CERT_FILE"));
    }
}

#[test]
fn empty_or_malformed_ca_bundle_fails_before_network() {
    let temp = tempfile::tempdir().unwrap();
    let bundle = temp.path().join("invalid.pem");
    for contents in [
        "",
        "not a certificate: private-fixture-marker",
        "-----BEGIN CERTIFICATE-----\n%%%\n-----END CERTIFICATE-----\n",
        "-----BEGIN CERTIFICATE-----\naGVsbG8=\n-----END CERTIFICATE-----\n",
    ] {
        fs::write(&bundle, contents).unwrap();
        let error = config_from_ca_file(Some(&bundle)).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("SSL_CERT_FILE"));
        assert!(!message.contains("private-fixture-marker"));
    }
}

#[test]
fn malformed_pem_markers_do_not_expose_bundle_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let bundle = temp.path().join("private-fixture-path.pem");
    for contents in [
        "-----BEGIN private-fixture-marker----\n",
        "-----BEGIN private-fixture-marker-----\naGVsbG8=\n",
    ] {
        fs::write(&bundle, contents).unwrap();
        let error = config_from_ca_file(Some(&bundle)).unwrap_err();
        // The underlying parser includes raw line bytes or the missing end
        // marker. Check the entire error chain to prevent either from leaking.
        assert_eq!(
            format!("{error:#}"),
            "cannot parse SSL_CERT_FILE PEM CA bundle"
        );
    }
}

#[test]
fn explicitly_empty_ca_path_is_rejected() {
    let error = config_from_ca_file(Some(Path::new(""))).unwrap_err();
    assert_eq!(format!("{error:#}"), "SSL_CERT_FILE must not be empty");
}

#[test]
fn oversized_ca_bundle_is_rejected_before_parsing() {
    let temp = tempfile::tempdir().unwrap();
    let bundle = temp.path().join("oversized.pem");
    fs::File::create(&bundle)
        .unwrap()
        .set_len(super::MAX_CA_BUNDLE_BYTES + 1)
        .unwrap();
    let error = config_from_ca_file(Some(&bundle)).unwrap_err();
    assert_eq!(
        format!("{error:#}"),
        "SSL_CERT_FILE CA bundle exceeds 16 MiB"
    );
}

#[test]
fn invalid_certificate_is_not_silently_ignored_in_an_otherwise_valid_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let bundle = temp.path().join("mixed.pem");
    let authority = Authority::new();
    fs::write(
        &bundle,
        format!(
            "{}-----BEGIN CERTIFICATE-----\naGVsbG8=\n-----END CERTIFICATE-----\n",
            authority.certificate.pem()
        ),
    )
    .unwrap();
    assert!(config_from_ca_file(Some(&bundle)).is_err());
}
