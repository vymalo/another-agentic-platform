#!/bin/sh
# The test CA and the leaves the conformance suite uses (ANR-1010). Run once, from this directory, with
# OpenSSL 3.5 or later; the certificates are checked in and the keys are not needed afterwards.
#
# Every leaf is valid in exactly one respect but one, so each refusal has a single cause:
#   accepted.pem     the trusted CA, client authentication, URI SAN spiffe://another/ns/test/sa/orchestrator
#   unknown.pem      the trusted CA, client authentication, a URI SAN the configuration does not name
#   wrong-use.pem    the trusted CA, server authentication instead of client
#   expired.pem      the trusted CA, client authentication, notAfter in 2020
#   other-ca-leaf.pem  another CA, client authentication, the accepted SAN
set -eu
cd "$(dirname "$0")"

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout ca.key -out ca.pem -days 3650 -subj "/CN=aap test CA" \
    -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign,cRLSign"

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout other-ca.key -out other-ca.pem -days 3650 -subj "/CN=aap test CA (other)" \
    -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign,cRLSign"

sign() {
    name=$1
    san=$2
    eku=$3
    extra=$4
    openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
        -keyout "$name.key" -out "$name.csr" -subj "/CN=$name"
    printf 'basicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=%s\nsubjectAltName=URI:%s\n' \
        "$eku" "$san" > "$name.ext"
    # shellcheck disable=SC2086
    openssl x509 -req -in "$name.csr" -CA ca.pem -CAkey ca.key -CAcreateserial \
        -out "$name.pem" -days 365 -extfile "$name.ext" $extra
}

sign accepted spiffe://another/ns/test/sa/orchestrator clientAuth ""
sign unknown "spiffe://another/ns/test/sa/aap-sentinel-never-a-value-8f3c1d" clientAuth ""
sign wrong-use spiffe://another/ns/test/sa/orchestrator serverAuth ""
sign expired spiffe://another/ns/test/sa/orchestrator clientAuth \
    "-not_before 20200101000000Z -not_after 20200102000000Z"

openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout other-ca-leaf.key -out other-ca-leaf.csr -subj "/CN=other-ca-leaf"
printf 'basicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=clientAuth\nsubjectAltName=URI:spiffe://another/ns/test/sa/orchestrator\n' \
    > other-ca-leaf.ext
openssl x509 -req -in other-ca-leaf.csr -CA other-ca.pem -CAkey other-ca.key -CAcreateserial \
    -out other-ca-leaf.pem -days 365 -extfile other-ca-leaf.ext

rm -f ./*.key ./*.csr ./*.ext ./*.srl
ls -1 ./*.pem
