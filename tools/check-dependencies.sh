#!/bin/sh
# The dependency gates of the workspace (K5, §48a): TLS stays rustls with ring (kube's provider) and nothing else brings a second TLS
# stack or a second HTTP client into the lock file. Needs `cargo`. In CI (operator.yml, job `dependencies`) with --locked; here
# offline is fine:
#
#   CARGO_FLAGS=--offline sh tools/check-dependencies.sh
#   CARGO_FLAGS=--offline BASE_REF=origin/main ALLOW_NEW_PACKAGES="a-package another" sh tools/check-dependencies.sh   # gate 5, local
#
# 1. rustls has exactly the features log, logging, ring, std, tls12: never aws_lc_rs.
# 2. The operator has no aws-lc-rs at all, and aws-lc-rs has one reason to be there: jsonwebtoken, through aap-auth-oidc.
# 3. No native-tls and no openssl-sys.
# 4. One reqwest, 0.12 (0.13's `rustls` feature is aws-lc-rs).
# 5. (opt-in, local use: only when BASE_REF is given; NOT run in CI, where a standing list of packages would fail every later pull
#    request) the lock file gains no package beyond those in ALLOW_NEW_PACKAGES, compared with the merge base of BASE_REF and HEAD.
#
# A gate that says "absent" accepts exactly one outcome: `cargo tree -i <name>` exiting 101 with the error line
# "package ID specification `<name>` did not match any packages" (verified 2026-11-15 on cargo 1.94 here: that is what it prints and
# returns for a package that is not in the graph). Any other failure (a --locked refusal, a bad flag, a package the command cannot
# find) is a failure of the gate, never a pass. The same message answers a misspelt name, so each "absent" gate also names a package
# it must find (the positive controls below) to show the command works on this graph.
set -eu

flags=${CARGO_FLAGS:---locked}
fail=0
say() { echo "$1"; }
bad() { echo "FAIL $1"; fail=1; }

# The one place `flags` is expanded: a list of words, on purpose.
ct() {
  # shellcheck disable=SC2086
  cargo tree $flags "$@"
}

err=$(mktemp)
trap 'rm -f "$err"' EXIT

# present <cargo tree args...>: prints the tree on stdout and returns 0 when the command succeeds, 3 when it fails. The failure
# text is in $err. (No pipe: the exit code of cargo itself is read.)
run() {
  if out=$(ct "$@" 2> "$err"); then
    return 0
  fi
  return 3
}

# absent <name> <cargo tree args...>: 0 when `cargo tree <args> -i <name>` says exactly that the package is not in the graph,
# 1 when the package is in it, 2 when the command failed any other way (the text is in $err).
absent() {
  name=$1
  shift
  if run "$@" -i "$name"; then
    return 1
  fi
  if [ "$(grep -c '^error' "$err")" -eq 1 ] \
    && grep -Fxq "error: package ID specification \`$name\` did not match any packages" "$err"; then
    return 0
  fi
  return 2
}

# The names this script asks about must be real: a package of the graph is found by the same command.
control() {
  if run -i "$1" && [ -n "$out" ]; then say "ok   control: $1 is found in the graph"; else bad "control: $1 is not found ($(head -c 300 "$err"))"; fi
}

# 1. Only rustls's own `rustls feature "x"` lines count: `hyper-rustls feature` and `tokio-rustls feature` are other crates'.
if run -e features,normal -i rustls; then
  tree=$out
  features=$(printf '%s\n' "$tree" | grep -Eo '(^|[^-a-z_])rustls feature "[^"]*"' | sed 's/.*feature "\(.*\)"/\1/' | sort -u | tr '\n' ' ')
  want='log logging ring std tls12 '
  if [ "$features" = "$want" ]; then say "ok   rustls has exactly the features: $features"; else bad "rustls features are '$features', expected '$want'"; fi
  if printf '%s\n' "$tree" | grep -Eq 'aws_lc_rs|aws-lc-rs'; then bad "rustls reaches aws-lc-rs"; else say "ok   rustls does not reach aws-lc-rs"; fi
else
  bad "cargo tree for rustls failed: $(head -c 300 "$err")"
fi

# 2
if run -p aap-operator -e normal --depth 0; then
  if absent aws-lc-rs -p aap-operator -e normal; then
    say "ok   the operator has no aws-lc-rs"
  else
    case $? in
      1) bad "the operator has aws-lc-rs" ;;
      *) bad "the check for aws-lc-rs in the operator failed: $(head -c 300 "$err")" ;;
    esac
  fi
else
  bad "the operator package is not found: $(head -c 300 "$err")"
fi
if run -e normal -i aws-lc-rs; then
  paths=$out
  if printf '%s\n' "$paths" | grep -Eq 'tokio-rustls|hyper-rustls|rustls v'; then bad "aws-lc-rs is reached through a TLS crate"; else say "ok   aws-lc-rs is reached through no TLS crate"; fi
  if printf '%s\n' "$paths" | grep -q 'jsonwebtoken' && printf '%s\n' "$paths" | grep -q 'aap-auth-oidc'; then say "ok   aws-lc-rs: jsonwebtoken, through aap-auth-oidc"; else bad "aws-lc-rs has another reason to be here: $paths"; fi
else
  bad "cargo tree for aws-lc-rs failed (it is expected in the graph, through jsonwebtoken): $(head -c 300 "$err")"
fi

# 3
control reqwest
for p in native-tls openssl-sys; do
  if absent "$p"; then
    say "ok   no $p"
  else
    case $? in
      1) bad "$p is in the dependency graph" ;;
      *) bad "the check for $p failed: $(head -c 300 "$err")" ;;
    esac
  fi
done

# 4
if run -i reqwest; then
  versions=$(printf '%s\n' "$out" | grep -Eo '^reqwest v[0-9.]+' | sort -u)
  if [ "$(printf '%s\n' "$versions" | wc -l)" -eq 1 ] && printf '%s\n' "$versions" | grep -Eq '^reqwest v0\.12\.[0-9]+$'; then
    say "ok   one reqwest: $versions"
  else
    bad "reqwest is '$versions': one 0.12"
  fi
else
  bad "cargo tree for reqwest failed: $(head -c 300 "$err")"
fi

# 5 (opt-in)
if [ -n "${BASE_REF:-}" ]; then
  if ! git rev-parse --verify --quiet "$BASE_REF^{commit}" > /dev/null; then
    bad "BASE_REF '$BASE_REF' is not a commit of this repository"
  elif ! base=$(git merge-base "$BASE_REF" HEAD); then
    bad "no merge base of '$BASE_REF' and HEAD"
  else
    added=$(git diff "$base" -- Cargo.lock | grep '^+name = ' | sed 's/^+name = "\(.*\)"/\1/' | sort -u | tr '\n' ' ')
    extra=""
    for n in $added; do
      case " ${ALLOW_NEW_PACKAGES:-} " in *" $n "*) ;; *) extra="$extra $n" ;; esac
    done
    if [ -z "$extra" ]; then say "ok   the lock file gains only: ${added:-nothing}"; else bad "the lock file gains packages that were not decided on:$extra"; fi
  fi
fi

if [ "$fail" -eq 0 ]; then echo "dependency gates passed"; else echo "dependency gates FAILED"; exit 1; fi
