//! The closed words of the client-certificate seam and their classes (ANR-1010): the counterpart of
//! `tests/objects.rs`'s token-word and token-class tests, for `InvalidCertificate`/`ClientAuthError`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use aap_ports::{Certificate, Classify, ClientAuthError, ErrorClass, InvalidCertificate};

#[test]
fn the_reason_a_certificate_is_invalid_is_one_of_a_fixed_set_of_words() {
    let words = [
        (InvalidCertificate::Malformed, "malformed"),
        (InvalidCertificate::BadChain, "bad chain"),
        (InvalidCertificate::WrongUse, "wrong use"),
        (InvalidCertificate::Expired, "expired"),
        (InvalidCertificate::UnknownIdentity, "unknown identity"),
        (InvalidCertificate::NoIdentity, "no identity"),
    ];
    // The set is exhaustive: a new variant is a word, a row and a test.
    assert_eq!(words.len(), InvalidCertificate::ALL.len());
    for (reason, word) in words {
        assert_eq!(reason.as_str(), word);
        assert_eq!(reason.to_string(), word);
        assert!(
            word.chars().all(|c| c.is_ascii_lowercase() || c == ' '),
            "{word} is not lowercase ASCII with spaces"
        );
    }
    for reason in InvalidCertificate::ALL {
        assert!(words.iter().any(|(r, _)| *r == reason), "{reason:?}");
    }
}

#[test]
fn every_client_auth_error_has_a_class_and_none_holds_a_certificate() {
    assert_eq!(ClientAuthError::Missing.class(), ErrorClass::Invalid);
    for reason in InvalidCertificate::ALL {
        assert_eq!(
            ClientAuthError::Invalid(reason).class(),
            ErrorClass::Invalid,
            "{reason:?}"
        );
    }
    let unavailable = ClientAuthError::Unavailable(Box::new(std::io::Error::other("a source")));
    assert_eq!(unavailable.class(), ErrorClass::Transient);
    // A certificate has no printable form that holds its bytes.
    let certificate = Certificate::new(b"a certificate's bytes".to_vec());
    let debug = format!("{certificate:?}");
    assert!(!debug.contains("certificate's"), "{debug}");
    assert!(debug.starts_with("Certificate("), "{debug}");
}
