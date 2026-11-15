#!/bin/sh
# The dependency gates of the workspace (K5, §48a): TLS stays rustls with ring (kube's provider) and nothing else brings a second TLS
# stack or a second HTTP client into the lock file. Needs `cargo`. In CI (operator.yml, job `dependencies`) with --locked; here
# offline is fine:
#
#   CARGO_FLAGS=--offline sh tools/check-dependencies.sh
#   CARGO_FLAGS=--offline ALLOW_NEW_PACKAGES="aap-trace aap-trace-otlp" BASE_REF=origin/main sh tools/check-dependencies.sh
#
# 1. rustls has exactly the features log, logging, ring, std, tls12: never aws_lc_rs.
# 2. The operator has no aws-lc-rs at all, and aws-lc-rs has one reason to be there: jsonwebtoken, through aap-auth-oidc.
# 3. No native-tls and no openssl-sys.
# 4. One reqwest, 0.12 (0.13's `rustls` feature is aws-lc-rs).
# 5. (only with ALLOW_NEW_PACKAGES, for a pull request) the lock file gains no package beyond those named, compared with BASE_REF.
#    The slice that adds a package says so in its pull request and edits the list in the workflow.
set -eu

# shellcheck disable=SC2086 # `flags` is a list of words, on purpose
flags=${CARGO_FLAGS:---locked}
fail=0
say() { echo "$1"; }
bad() { echo "FAIL $1"; fail=1; }

# 1. Only rustls's own `rustls feature "x"` lines count: `hyper-rustls feature` and `tokio-rustls feature` are other crates'.
features=$(cargo tree $flags -e features,normal -i rustls 2>/dev/null | grep -Eo '(^|[^-a-z_])rustls feature "[^"]*"' | sed 's/.*feature "\(.*\)"/\1/' | sort -u | tr '\n' ' ')
want='log logging ring std tls12 '
if [ "$features" = "$want" ]; then say "ok   rustls has exactly the features: $features"; else bad "rustls features are '$features', expected '$want'"; fi
if cargo tree $flags -e features,normal -i rustls 2>/dev/null | grep -Eq 'aws_lc_rs|aws-lc-rs'; then bad "rustls reaches aws-lc-rs"; else say "ok   rustls does not reach aws-lc-rs"; fi

# 2
if cargo tree $flags -p aap-operator -e normal -i aws-lc-rs >/dev/null 2>&1; then bad "the operator has aws-lc-rs"; else say "ok   the operator has no aws-lc-rs"; fi
paths=$(cargo tree $flags -e normal -i aws-lc-rs 2>/dev/null)
if printf '%s\n' "$paths" | grep -Eq 'tokio-rustls|hyper-rustls|rustls v'; then bad "aws-lc-rs is reached through a TLS crate"; else say "ok   aws-lc-rs is reached through no TLS crate"; fi
if printf '%s\n' "$paths" | grep -q 'jsonwebtoken' && printf '%s\n' "$paths" | grep -q 'aap-auth-oidc'; then say "ok   aws-lc-rs: jsonwebtoken, through aap-auth-oidc"; else bad "aws-lc-rs has another reason to be here: $paths"; fi

# 3
for p in native-tls openssl-sys; do
  if cargo tree $flags -i "$p" >/dev/null 2>&1; then bad "$p is in the dependency graph"; else say "ok   no $p"; fi
done

# 4
versions=$(cargo tree $flags -i reqwest 2>/dev/null | grep -Eo '^reqwest v[0-9.]+' | sort -u)
if [ "$(printf '%s\n' "$versions" | wc -l)" -eq 1 ] && printf '%s\n' "$versions" | grep -Eq '^reqwest v0\.12\.[0-9]+$'; then
  say "ok   one reqwest: $versions"
else
  bad "reqwest is '$versions': one 0.12"
fi

# 5
if [ -n "${ALLOW_NEW_PACKAGES:-}" ]; then
  base=${BASE_REF:-origin/main}
  added=$(git diff "$base" -- Cargo.lock | grep '^+name = ' | sed 's/^+name = "\(.*\)"/\1/' | sort -u | tr '\n' ' ')
  extra=""
  for n in $added; do
    case " $ALLOW_NEW_PACKAGES " in *" $n "*) ;; *) extra="$extra $n" ;; esac
  done
  if [ -z "$extra" ]; then say "ok   the lock file gains only: ${added:-nothing}"; else bad "the lock file gains packages that were not decided on:$extra"; fi
fi

if [ "$fail" -eq 0 ]; then echo "dependency gates passed"; else echo "dependency gates FAILED"; exit 1; fi
