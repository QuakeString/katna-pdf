#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Makes the test PKI for revocation checks, with OpenSSL: a root, an
# intermediate under it, "Meera Iyer" (good) and "Raj Kumar" (revoked)
# under that, an OCSP responder, the OCSP answers for both, and CRLs.
# The certificates point to http://pki.katna.test/ for their issuers,
# OCSP and CRLs; the tests answer for that host themselves.
set -eu
cd "$(dirname "$0")"
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
U=http://pki.katna.test
DAYS=36500

cat > "$W/ext.cnf" <<CNF
[root]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
[inter]
basicConstraints = critical, CA:TRUE, pathlen:0
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
authorityInfoAccess = caIssuers;URI:$U/root.cer
crlDistributionPoints = URI:$U/root.crl
[signer]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature, nonRepudiation
authorityKeyIdentifier = keyid
authorityInfoAccess = OCSP;URI:$U/ocsp, caIssuers;URI:$U/inter.cer
crlDistributionPoints = URI:$U/inter.crl
[ocsp]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, OCSPSigning
CNF

ca_conf() { # name dir
	cat > "$W/$1.cnf" <<CNF
[ca]
default_ca = this
[this]
database = $W/$1-index.txt
serial = $W/$1-serial
certificate = $W/$1.pem
private_key = $W/$1.key
default_md = sha256
default_crl_days = $DAYS
crlnumber = $W/$1-crlnumber
crl_extensions = crl_ext
[crl_ext]
authorityKeyIdentifier = keyid
CNF
	echo 1000 > "$W/$1-crlnumber"
	: > "$W/$1-index.txt"
}

key() { openssl genpkey -quiet -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$W/$1.key"; }

key root
openssl req -quiet -new -x509 -key "$W/root.key" -subj "/C=IN/O=Katna Test PKI/CN=Katna Test PKI Root" \
	-days $DAYS -sha256 -extensions root -config "$W/ext.cnf" -out "$W/root.pem" 2>/dev/null \
	|| openssl req -new -x509 -key "$W/root.key" -subj "/C=IN/O=Katna Test PKI/CN=Katna Test PKI Root" \
	-days $DAYS -sha256 -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" -out "$W/root.pem"

issue() { # name subject issuer section serial
	key "$1"
	openssl req -new -key "$W/$1.key" -subj "$2" -out "$W/$1.csr"
	openssl x509 -req -in "$W/$1.csr" -CA "$W/$3.pem" -CAkey "$W/$3.key" -set_serial "$5" \
		-days $DAYS -sha256 -extfile "$W/ext.cnf" -extensions "$4" -out "$W/$1.pem" 2>/dev/null
}

issue inter "/C=IN/O=Katna Test PKI/CN=Katna Test PKI Signing CA" root inter 4096
issue meera "/C=IN/O=Katna Test PKI/CN=Meera Iyer" inter signer 8193
issue raj "/C=IN/O=Katna Test PKI/CN=Raj Kumar" inter signer 8194
issue ocsp "/C=IN/O=Katna Test PKI/CN=Katna Test PKI OCSP" inter ocsp 8195

# The databases the CRLs and OCSP answers come from: Raj revoked.
ca_conf root
ca_conf inter
openssl ca -config "$W/inter.cnf" -valid "$W/meera.pem" 2>/dev/null
openssl ca -config "$W/inter.cnf" -valid "$W/raj.pem" 2>/dev/null
openssl ca -config "$W/inter.cnf" -revoke "$W/raj.pem" -crl_reason keyCompromise 2>/dev/null
openssl ca -config "$W/root.cnf" -valid "$W/inter.pem" 2>/dev/null
openssl ca -config "$W/inter.cnf" -gencrl -out "$W/inter.crl.pem" 2>/dev/null
openssl ca -config "$W/root.cnf" -gencrl -out "$W/root.crl.pem" 2>/dev/null

for who in meera raj; do
	openssl ocsp -issuer "$W/inter.pem" -cert "$W/$who.pem" -no_nonce -reqout "$W/$who.req" 2>/dev/null
	openssl ocsp -index "$W/inter-index.txt" -rsigner "$W/ocsp.pem" -rkey "$W/ocsp.key" -CA "$W/inter.pem" \
		-ndays $DAYS -reqin "$W/$who.req" -respout "$who.ocsp" 2>/dev/null
	# The key and its certificate only: the chain is found online.
	openssl pkcs12 -export -inkey "$W/$who.key" -in "$W/$who.pem" -passout pass:katna \
		-name "$who" -out "$who.p12"
done
for c in root inter meera raj; do
	openssl x509 -in "$W/$c.pem" -outform DER -out "$c.cer"
done
openssl crl -in "$W/inter.crl.pem" -outform DER -out inter.crl
openssl crl -in "$W/root.crl.pem" -outform DER -out root.crl
