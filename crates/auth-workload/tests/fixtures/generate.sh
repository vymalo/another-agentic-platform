#!/bin/sh
# The test CA and the leaves the conformance suite uses (ANR-1010). Run once, from this directory, with
# OpenSSL 3.5 or later; the certificates are checked in and the keys are not needed afterwards.
#
# Every leaf is valid in exactly one respect but one, so each refusal has a single cause:
#   accepted.pem        the trusted CA, client authentication, URI SAN .../orchestrator
#   no-permissions.pem  the trusted CA, client authentication, URI SAN .../no-permissions
#   unknown.pem         the trusted CA, client authentication, a URI SAN the configuration does not name
#   no-eku.pem          the trusted CA, NO extended key usage extension, the accepted URI SAN
#   no-identity.pem     the trusted CA, client authentication, a DNS SAN only
#   two-sans.pem        the trusted CA, client authentication, TWO URI SANs
#   wrong-use.pem       the trusted CA, server authentication instead of client
#   expired.pem         the trusted CA, client authentication, notAfter in 2020
#   other-ca-leaf.pem   another CA, client authentication, the accepted URI SAN
#
# The valid leaves live 100 years: they are checked in, and a suite that went red on a date would read
# like a regression in the authenticator.
set -eu
cd "$(dirname "$0")"

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout ca.key -out ca.pem -days 36500 -subj "/CN=aap test CA" \
    -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign,cRLSign"

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout other-ca.key -out other-ca.pem -days 36500 -subj "/CN=aap test CA (other)" \
    -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign,cRLSign"

# sign <name> <ca-cert> <ca-key> <days> <extensions...>
sign() {
    name=$1
    ca=$2
    ca_key=$3
    days=$4
    shift 4
    openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
        -keyout "$name.key" -out "$name.csr" -subj "/CN=$name"
    : > "$name.ext"
    for line in "$@"; do
        printf '%s\n' "$line" >> "$name.ext"
    done
    # shellcheck disable=SC2086
    openssl x509 -req -in "$name.csr" -CA "$ca" -CAkey "$ca_key" -CAcreateserial \
        -out "$name.pem" -days "$days" -extfile "$name.ext"
}

sign accepted ca.pem ca.key 36500 \
    "basicConstraints=CA:FALSE" "keyUsage=critical,digitalSignature" \
    "extendedKeyUsage=clientAuth" \
    "subjectAltName=URI:spiffe://another/ns/test/sa/orchestrator"

sign no-permissions ca.pem ca.key 36500 \
    "basicConstraints=CA:FALSE" "keyUsage=critical,digitalSignature" \
    "extendedKeyUsage=clientAuth" \
    "subjectAltName=URI:spiffe://another/ns/test/sa/no-permissions"

sign unknown ca.pem ca.key 36500 \
    "basicConstraints=CA:FALSE" "keyUsage=critical,digitalSignature" \
    "extendedKeyUsage=clientAuth" \
    "subjectAltName=URI:spiffe://another/ns/test/sa/aap-sentinel-never-a-value-8f3c1d"

sign no-eku ca.pem ca.key 36500 \
    "basicConstraints=CA:FALSE" "keyUsage=critical,digitalSignature" \
    "subjectAltName=URI:spiffe://another/ns/test/sa/orchestrator"

sign no-identity ca.pem ca.key 36500 \
    "basicConstraints=CA:FALSE" "keyUsage=critical,digitalSignature" \
    "extendedKeyUsage=clientAuth" \
    "subjectAltName=DNS:orchestrator.test"

sign two-sans ca.pem ca.key 36500 \
    "basicConstraints=CA:FALSE" "keyUsage=critical,digitalSignature" \
    "extendedKeyUsage=clientAuth" \
    "subjectAltName=URI:spiffe://another/ns/test/sa/orchestrator,URI:spiffe://another/ns/test/sa/other"

sign wrong-use ca.pem ca.key 36500 \
    "basicConstraints=CA:FALSE" "keyUsage=critical,digitalSignature" \
    "extendedKeyUsage=serverAuth" \
    "subjectAltName=URI:spiffe://another/ns/test/sa/orchestrator"

sign expired ca.pem ca.key 36500 \
    "basicConstraints=CA:FALSE" "keyUsage=critical,digitalSignature" \
    "extendedKeyUsage=clientAuth" \
    "subjectAltName=URI:spiffe://another/ns/test/sa/orchestrator"
# The expired leaf is re-signed with an explicit 2020 window; `-days` above is overridden below.
openssl x509 -req -in expired.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
    -out expired.pem -not_before 20200101000000Z -not_after 20200102000000Z -extfile expired.ext

sign other-ca-leaf other-ca.pem other-ca.key 36500 \
    "basicConstraints=CA:FALSE" "keyUsage=critical,digitalSignature" \
    "extendedKeyUsage=clientAuth" \
    "subjectAltName=URI:spiffe://another/ns/test/sa/orchestrator"

rm -f ./*.key ./*.csr ./*.ext ./*.srl
ls -1 ./*.pem
