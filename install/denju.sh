#!/bin/sh
set -eu

owner="amxv"
repo="denju"
manifest_name="release-manifest.txt"
install_dir="${DENJU_INSTALL_DIR:-$HOME/.local/bin}"

download() {
  url=$1
  output=$2
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 3 --connect-timeout 10 -o "$output" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$output" "$url"
  else
    echo "denju installer requires curl or wget" >&2
    exit 1
  fi
}

sha256_file() {
  path=$1
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$path" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$path" | awk '{print $NF}'
  else
    echo "denju installer requires sha256sum, shasum, or openssl" >&2
    exit 1
  fi
}

case "$(uname -s)" in
  Darwin) os=darwin ;;
  Linux) os=linux ;;
  *) echo "Unsupported Denju operating system: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64) arch=amd64 ;;
  arm64|aarch64) arch=arm64 ;;
  *) echo "Unsupported Denju architecture: $(uname -m)" >&2; exit 1 ;;
esac
asset="denju_${os}_${arch}"

tmp_root=${TMPDIR:-/tmp}
tmp_dir=$(mktemp -d "$tmp_root/denju-install.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

if [ -n "${DENJU_RELEASE_BASE_URL:-}" ]; then
  release_base=${DENJU_RELEASE_BASE_URL%/}
  manifest_url="$release_base/$manifest_name"
elif [ -n "${DENJU_VERSION:-}" ]; then
  release_base="https://github.com/$owner/$repo/releases/download/v$DENJU_VERSION"
  manifest_url="$release_base/$manifest_name"
else
  manifest_url="https://github.com/$owner/$repo/releases/latest/download/$manifest_name"
  release_base=""
fi

manifest="$tmp_dir/$manifest_name"
download "$manifest_url" "$manifest"
record=$(awk -v selected="$asset" '
  BEGIN {
    known["denju_darwin_amd64"] = 1
    known["denju_darwin_arm64"] = 1
    known["denju_linux_amd64"] = 1
    known["denju_linux_arm64"] = 1
    known["denju_windows_amd64.exe"] = 1
    known["denju_windows_arm64.exe"] = 1
  }
  function invalid() { bad = 1; exit 2 }
  NF == 0 { next }
  $1 == "format" {
    if (NF != 2 || ++format_count != 1) invalid()
    format = $2
    next
  }
  $1 == "version" {
    if (NF != 2 || ++version_count != 1) invalid()
    version = $2
    next
  }
  $1 == "asset" {
    if (NF != 4 || !($2 in known) || seen_asset[$2]++) invalid()
    if (length($3) != 64 || $3 ~ /[^0-9A-Fa-f]/ || $4 !~ /^[0-9]+$/) invalid()
    asset_count++
    if ($2 == selected) {
      selected_sha = tolower($3)
      selected_size = $4
    }
    next
  }
  $1 == "server_image" {
    if (NF != 2 || ++server_count != 1) invalid()
    server_image = $2
    next
  }
  { invalid() }
  END {
    if (bad) exit 2
    if (format_count != 1 || format != "denju-release-manifest-v1") exit 2
    if (version_count != 1 || length(version) < 1 || length(version) > 64 || version ~ /[^-A-Za-z0-9.+]/) exit 2
    if (asset_count != 6 || selected_sha == "" || selected_size == "") exit 2
    if (server_count != 1 || server_image != "ghcr.io/amxv/denju-server:v" version) exit 2
    printf "%s|%s|%s\n", version, selected_sha, selected_size
  }
' "$manifest") || {
  echo "Invalid Denju release manifest" >&2
  exit 1
}
version=$(printf '%s\n' "$record" | cut -d '|' -f 1)
expected_sha=$(printf '%s\n' "$record" | cut -d '|' -f 2)
expected_size=$(printf '%s\n' "$record" | cut -d '|' -f 3)
if [ -n "${DENJU_VERSION:-}" ] && [ "$DENJU_VERSION" != "$version" ]; then
  echo "Release manifest version $version does not match requested $DENJU_VERSION" >&2
  exit 1
fi
if [ -z "$release_base" ]; then
  release_base="https://github.com/$owner/$repo/releases/download/v$version"
fi

staged="$tmp_dir/$asset"
download "$release_base/$asset" "$staged"
actual_size=$(wc -c < "$staged" | tr -d ' ')
if [ "$actual_size" != "$expected_size" ]; then
  echo "Size mismatch for $asset" >&2
  exit 1
fi
actual_sha=$(sha256_file "$staged")
if [ "$actual_sha" != "$expected_sha" ]; then
  echo "Checksum mismatch for $asset" >&2
  exit 1
fi

mkdir -p "$install_dir"
target="$install_dir/denju"
chmod 755 "$staged"
mv "$staged" "$target.tmp.$$"
mv "$target.tmp.$$" "$target"

mkdir -p "$HOME/.denju"
printf '%s\n' '{"version":1,"source":"standalone"}' > "$HOME/.denju/install-source.json.tmp.$$"
mv "$HOME/.denju/install-source.json.tmp.$$" "$HOME/.denju/install-source.json"

echo "Installed denju $version to $target"
case ":${PATH:-}:" in
  *":$install_dir:"*) ;;
  *) echo "Add $install_dir to PATH, then run: denju setup" ;;
esac
