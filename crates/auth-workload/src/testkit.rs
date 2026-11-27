//! A PKI minted at run time, for tests that need a handshake (ANR-1011).
//!
//! The leaves checked in under `tests/fixtures/` (ANR-1010) have no private key, so no client can use them to complete a TLS
//! handshake: their keys were thrown away on purpose. This module makes a CA and leaves **with** keys, in memory, for the
//! socket test of the listener. Nothing here is a secret, nothing is written to disk by this module, and the feature is in no
//! image.

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, SanType, date_time_ymd,
};

/// A CA that signs leaves.
pub struct TestPki {
    issuer: CertifiedIssuer<'static, KeyPair>,
    /// The intermediates a leaf of this CA brings, DER, its own issuer first: empty for a root, this CA's certificate and then its
    /// parent's for an intermediate.
    chain: Vec<Vec<u8>>,
}

/// What a leaf is made of. Every field is one thing a refusal depends on.
#[derive(Clone, Debug)]
pub struct LeafSpec {
    /// The URI SANs (SPIFFE ids).
    pub uris: Vec<String>,
    /// The DNS and IP SANs (a server certificate).
    pub hosts: Vec<String>,
    /// The extended key usages. **Empty means the extension is absent**, which `webpki` accepts and the authenticator must not.
    pub usages: Vec<Usage>,
    /// `notAfter` as (year, month, day).
    pub not_after: (i32, u8, u8),
}

/// An extended key usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    /// `id-kp-clientAuth`.
    Client,
    /// `id-kp-serverAuth`.
    Server,
}

impl LeafSpec {
    /// A client leaf for `spiffe_id`, valid until 2099.
    pub fn client(spiffe_id: &str) -> Self {
        Self {
            uris: vec![spiffe_id.to_owned()],
            hosts: Vec::new(),
            usages: vec![Usage::Client],
            not_after: (2099, 1, 1),
        }
    }

    /// A server leaf for `hosts` (names or IP addresses), valid until 2099.
    pub fn server(hosts: &[&str]) -> Self {
        Self {
            uris: Vec::new(),
            hosts: hosts.iter().map(|h| (*h).to_owned()).collect(),
            usages: vec![Usage::Server],
            not_after: (2099, 1, 1),
        }
    }
}

/// A leaf and its key.
pub struct Leaf {
    /// The certificate, DER.
    pub cert_der: Vec<u8>,
    /// The certificate, PEM.
    pub cert_pem: String,
    /// The private key, PKCS#8 DER.
    pub key_der: Vec<u8>,
    /// The private key, PEM (what a file holds).
    pub key_pem: String,
    /// The intermediate CAs between this leaf and the root, DER, the leaf's issuer first. Empty when the root issued it. A peer sends
    /// them after the leaf.
    pub intermediates: Vec<Vec<u8>>,
}

impl TestPki {
    /// A new CA named `name`.
    ///
    /// # Panics
    ///
    /// When the generator fails: this is test tooling.
    #[allow(clippy::expect_used)]
    pub fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("empty SANs");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.distinguished_name.push(DnType::CommonName, name);
        params.not_after = date_time_ymd(2099, 1, 1);
        let key = KeyPair::generate().expect("a key");
        Self {
            issuer: CertifiedIssuer::self_signed(params, key).expect("a self-signed CA"),
            chain: Vec::new(),
        }
    }

    /// An intermediate CA named `name`, signed by this one: a leaf it issues brings it (and this CA's own intermediates), and **the
    /// bundle that trusts the root alone** accepts the leaf. This is the shape of a PKI that issues from an intermediate.
    ///
    /// # Panics
    ///
    /// When the generator fails: this is test tooling.
    #[allow(clippy::expect_used)]
    pub fn intermediate(&self, name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("empty SANs");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.distinguished_name.push(DnType::CommonName, name);
        params.not_after = date_time_ymd(2099, 1, 1);
        let key = KeyPair::generate().expect("a key");
        let parent: &Issuer<'_, KeyPair> = &self.issuer;
        let issuer =
            CertifiedIssuer::signed_by(params, key, parent).expect("a signed intermediate CA");
        let mut chain = vec![issuer.der().to_vec()];
        chain.extend(self.chain.iter().cloned());
        Self { issuer, chain }
    }

    /// The CA certificate, PEM: the bundle a listener is configured with.
    pub fn ca_pem(&self) -> String {
        self.issuer.pem()
    }

    /// A leaf of `spec`, signed by this CA.
    ///
    /// # Panics
    ///
    /// When the generator fails: this is test tooling.
    #[allow(clippy::expect_used)]
    pub fn leaf(&self, spec: &LeafSpec) -> Leaf {
        let mut params = CertificateParams::new(spec.hosts.clone()).expect("host SANs");
        for uri in &spec.uris {
            params
                .subject_alt_names
                .push(SanType::URI(uri.clone().try_into().expect("an ASCII URI")));
        }
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = spec
            .usages
            .iter()
            .map(|u| match u {
                Usage::Client => ExtendedKeyUsagePurpose::ClientAuth,
                Usage::Server => ExtendedKeyUsagePurpose::ServerAuth,
            })
            .collect();
        params.distinguished_name.push(DnType::CommonName, "leaf");
        let (y, m, d) = spec.not_after;
        params.not_after = date_time_ymd(y, m, d);
        let key = KeyPair::generate().expect("a key");
        let issuer: &Issuer<'_, KeyPair> = &self.issuer;
        let cert = params.signed_by(&key, issuer).expect("a signed leaf");
        Leaf {
            cert_der: cert.der().to_vec(),
            cert_pem: cert.pem(),
            key_der: key.serialize_der(),
            key_pem: key.serialize_pem(),
            intermediates: self.chain.clone(),
        }
    }
}
