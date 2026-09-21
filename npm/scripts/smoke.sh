#!/usr/bin/env bash
#
# Verify the staged npm packages the way a user installs them: pack the real
# tarballs, install them into a throwaway project, and run the shim.
#
#   npm/scripts/smoke.sh <staging-dir>
#
# Checks that:
#   1. `hyper` and `ha` both resolve to the platform binary and report the same
#      version
#   2. a failing run exits non-zero through the shim (exit code passthrough)
#   3. a missing platform package produces an actionable error, not a stack trace
#
# Runs on the host platform only, so a CI matrix must include that platform.

set -euo pipefail

staging="${1:?usage: smoke.sh <staging-dir>}"
staging="$(cd "$staging" && pwd)"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
host="$(node -p 'process.platform + "-" + process.arch')"
main_name="$(node -e 'console.log(require(process.argv[1]).name)' "$script_dir/../package.json")"
main_dir="$staging/$main_name"
# Resolve the platform package through platforms.json: the package name is not
# always derivable from process.platform (the Windows package is named after the
# OS, see the note in npm/platforms.json).
platform_name="$(node -e '
  const { platforms } = require(process.argv[1]);
  const platform = platforms.find((p) => p.key === process.argv[2]);
  if (!platform) {
    console.error("smoke: no platform package declared for " + process.argv[2]);
    process.exit(1);
  }
  console.log(platform.name);
' "$script_dir/../platforms.json" "$host")"
platform_dir="$staging/$platform_name"

if [ ! -d "$main_dir" ] || [ ! -d "$platform_dir" ]; then
  echo "smoke: expected $main_dir and $platform_dir to exist" >&2
  exit 1
fi

version="$(node -e 'console.log(require(process.argv[1]).version)' "$main_dir/package.json")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "smoke: $main_name@$version on $host ($platform_name)"

mkdir -p "$work/packed/platform" "$work/packed/main"
(cd "$work/packed/platform" && npm pack --silent "$platform_dir" >/dev/null)
(cd "$work/packed/main" && npm pack --silent "$main_dir" >/dev/null)
platform_tgz="$work/packed/platform/$platform_name-$version.tgz"
main_tgz="$work/packed/main/$main_name-$version.tgz"
for tgz in "$platform_tgz" "$main_tgz"; do
  [ -f "$tgz" ] || { echo "smoke: npm pack did not produce $tgz" >&2; exit 1; }
done

echo "  tarball sizes: $(du -h "$platform_tgz" | cut -f1) platform, $(du -h "$main_tgz" | cut -f1) main"

# The binary must be executable inside the tarball, otherwise the installed shim
# would fail on a permission error rather than running.
mkdir -p "$work/inspect"
tar -xzf "$platform_tgz" -C "$work/inspect"
platform_bin="$work/inspect/package/bin/$(node -p "process.platform === 'win32' ? 'hyper.exe' : 'hyper'")"
[ -x "$platform_bin" ] || { echo "smoke: $platform_bin is not executable in the tarball" >&2; exit 1; }

mkdir -p "$work/install"
cd "$work/install"
npm init -y >/dev/null
npm install --no-audit --no-fund --omit=optional "$platform_tgz" "$main_tgz" >/dev/null

bin="$work/install/node_modules/.bin"
[ -x "$bin/hyper" ] || { echo "smoke: npm did not link the hyper binary" >&2; exit 1; }
[ -x "$bin/ha" ] || { echo "smoke: npm did not link the ha alias" >&2; exit 1; }

reported="$("$bin/hyper" --version)"
echo "  hyper --version -> $reported"
case "$reported" in
  *"$version"*) ;;
  *) echo "smoke: expected version $version in '$reported'" >&2; exit 1 ;;
esac

# `ha` is the same program under a shorter name, so it must answer identically.
aliased="$("$bin/ha" --version)"
echo "  ha --version -> $aliased"
[ "$aliased" = "$reported" ] || {
  echo "smoke: ha reported '$aliased' but hyper reported '$reported'" >&2
  exit 1
}

cat > task.json <<'JSON'
{"name":"smoke","steps":[{"id":"s","mode":"build","instruction":"bash:exit 7"}]}
JSON
status=0
"$bin/hyper" run task.json >out.json 2>err.txt || status=$?
if [ "$status" -ne 1 ]; then
  echo "smoke: a failing run must exit 1 through the shim, got $status" >&2
  exit 1
fi
grep -q '"status": "failed"' out.json || {
  echo "smoke: the run summary was not printed" >&2
  exit 1
}

# A user who lost the optional dependency must get advice, not a failure mode
# they cannot act on.
mkdir -p "$work/missing"
cd "$work/missing"
npm init -y >/dev/null
npm install --no-audit --no-fund --omit=optional "$main_tgz" >/dev/null
status=0
"$work/missing/node_modules/.bin/hyper" --version >out.txt 2>err.txt || status=$?
if [ "$status" -ne 1 ] || ! grep -q "HYPER_BINARY_PATH" err.txt; then
  echo "smoke: expected an actionable error for the missing platform package" >&2
  cat err.txt >&2
  exit 1
fi

echo "smoke: ok"
