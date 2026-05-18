#!/usr/bin/env bash

set -euo pipefail

REPO="${TOKENBOARD_REPO:-james-uea/tokenboard}"
VERSION="${TOKENBOARD_VERSION:-latest}"
INSTALL_DIR="${TOKENBOARD_INSTALL_DIR:-}"
TOKENBOARD_TMP_DIR=""

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/install.sh

Environment:
  TOKENBOARD_REPO         GitHub repo to download from. Default: james-uea/tokenboard
  TOKENBOARD_VERSION      Release tag to install, or "latest". Default: latest
  TOKENBOARD_INSTALL_DIR  Install directory. Default: /usr/local/bin or ~/.local/bin
  GITHUB_TOKEN            Optional token for authenticated direct release downloads
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "$1 is required" >&2
    exit 1
  }
}

detect_assets() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *)
      echo "Unsupported CPU architecture: $arch" >&2
      exit 1
      ;;
  esac

  case "$os" in
    Darwin)
      case "$arch" in
        aarch64) echo "tokenboard-aarch64-apple-darwin" ;;
        x86_64) echo "tokenboard-x86_64-apple-darwin" ;;
      esac
      ;;
    Linux)
      if [[ "$arch" != "x86_64" ]]; then
        echo "Linux release builds currently support x86_64 only." >&2
        exit 1
      fi
      echo "tokenboard-x86_64-unknown-linux-musl"
      echo "tokenboard-x86_64-unknown-linux-gnu"
      ;;
    MINGW*|MSYS*|CYGWIN*)
      if [[ "$arch" != "x86_64" ]]; then
        echo "Windows release builds currently support x86_64 only." >&2
        exit 1
      fi
      echo "tokenboard-x86_64-pc-windows-msvc.exe"
      ;;
    *)
      echo "Unsupported OS: $os" >&2
      exit 1
      ;;
  esac
}

choose_install_dir() {
  if [[ -n "$INSTALL_DIR" ]]; then
    echo "$INSTALL_DIR"
    return
  fi

  if [[ -d /usr/local/bin && -w /usr/local/bin ]]; then
    echo "/usr/local/bin"
    return
  fi

  if [[ -d /usr/local/bin ]] && command -v sudo >/dev/null 2>&1; then
    echo "/usr/local/bin"
    return
  fi

  echo "$HOME/.local/bin"
}

download_with_gh() {
  local asset="$1" tmp="$2"
  local args=(release download --repo "$REPO" --pattern "$asset" --dir "$tmp" --clobber)

  if [[ "$VERSION" != "latest" ]]; then
    args=(release download "$VERSION" --repo "$REPO" --pattern "$asset" --dir "$tmp" --clobber)
  fi

  gh "${args[@]}" || return 1

  local checksum_args=(release download --repo "$REPO" --pattern "$asset.sha256" --dir "$tmp" --clobber)
  if [[ "$VERSION" != "latest" ]]; then
    checksum_args=(release download "$VERSION" --repo "$REPO" --pattern "$asset.sha256" --dir "$tmp" --clobber)
  fi
  gh "${checksum_args[@]}" >/dev/null 2>&1 || true

  [[ -f "$tmp/$asset" ]]
}

can_use_gh() {
  command -v gh >/dev/null 2>&1 || return 1
  gh auth status >/dev/null 2>&1
}

curl_download() {
  local output="$1" url="$2"
  if [[ -n "${GITHUB_TOKEN:-}" ]]; then
    curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" -o "$output" "$url"
  else
    curl -fsSL -o "$output" "$url"
  fi
}

download_with_curl() {
  local asset="$1" tmp="$2"
  need curl

  local release_path="latest/download"
  if [[ "$VERSION" != "latest" ]]; then
    release_path="download/$VERSION"
  fi

  rm -f "$tmp/$asset" "$tmp/$asset.sha256"

  curl_download \
    "$tmp/$asset" \
    "https://github.com/$REPO/releases/$release_path/$asset" || return 1

  curl_download \
    "$tmp/$asset.sha256" \
    "https://github.com/$REPO/releases/$release_path/$asset.sha256" >/dev/null 2>&1 || true
}

sha256_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    return 1
  fi
}

verify_checksum() {
  local file="$1" checksum_file="$2"

  if [[ ! -f "$checksum_file" ]]; then
    echo "No checksum asset found; skipping checksum verification." >&2
    return
  fi

  local expected actual
  expected="$(awk '{print tolower($1)}' "$checksum_file")"
  actual="$(sha256_file "$file" | tr '[:upper:]' '[:lower:]')" || {
    echo "No sha256 tool found; skipping checksum verification." >&2
    return
  }

  if [[ -z "$expected" || "$expected" != "$actual" ]]; then
    echo "Checksum verification failed for $(basename "$file")." >&2
    exit 1
  fi

  echo "Checksum verified."
}

install_binary() {
  local src="$1" install_dir="$2" install_name="$3"
  local sudo_cmd=()

  if [[ "$install_dir" == /usr/local/bin && ! -w "$install_dir" ]]; then
    sudo_cmd=(sudo)
  fi

  if [[ "${#sudo_cmd[@]}" -gt 0 ]]; then
    "${sudo_cmd[@]}" mkdir -p "$install_dir"
    "${sudo_cmd[@]}" install -m 0755 "$src" "$install_dir/$install_name"
  else
    mkdir -p "$install_dir"
    install -m 0755 "$src" "$install_dir/$install_name"
  fi
}

main() {
  local asset candidate tmp install_dir install_name
  local assets=()
  while IFS= read -r candidate; do
    assets+=("$candidate")
  done < <(detect_assets)
  asset=""
  tmp="$(mktemp -d)"
  TOKENBOARD_TMP_DIR="$tmp"
  install_dir="$(choose_install_dir)"
  install_name="tokenboard"
  if [[ "$asset" == *.exe ]]; then
    install_name="tokenboard.exe"
  fi

  cleanup() {
    if [[ -n "$TOKENBOARD_TMP_DIR" ]]; then
      rm -rf "$TOKENBOARD_TMP_DIR"
    fi
  }
  trap cleanup EXIT

  echo "Installing Tokenboard from $REPO ($VERSION)"
  echo "Detected release assets: ${assets[*]}"

  for candidate in "${assets[@]}"; do
    echo "Trying release asset: $candidate"
    if can_use_gh; then
      if download_with_gh "$candidate" "$tmp"; then
        asset="$candidate"
        break
      fi
      echo "GitHub CLI download failed for $candidate; trying direct release download." >&2
      if download_with_curl "$candidate" "$tmp"; then
        asset="$candidate"
        break
      fi
    else
      if command -v gh >/dev/null 2>&1; then
        echo "GitHub CLI is installed but not authenticated; using direct release download." >&2
      fi
      if download_with_curl "$candidate" "$tmp"; then
        asset="$candidate"
        break
      fi
    fi
    echo "Release asset unavailable: $candidate" >&2
  done

  if [[ -z "$asset" ]]; then
    echo "No compatible Tokenboard release asset was found for this platform." >&2
    exit 1
  fi

  echo "Selected release asset: $asset"

  verify_checksum "$tmp/$asset" "$tmp/$asset.sha256"
  install_binary "$tmp/$asset" "$install_dir" "$install_name"

  local installed_path resolved_path
  installed_path="$install_dir/$install_name"

  echo "Installed $installed_path"
  if [[ -x "$installed_path" ]]; then
    "$installed_path" --version
    echo 'Next: run `tokenboard setup` to sign in with GitHub.'
    resolved_path="$(command -v "$install_name" 2>/dev/null || true)"
    if [[ -z "$resolved_path" ]]; then
      echo "$install_dir is not on PATH. Add it to PATH or run $installed_path directly." >&2
    elif [[ "$resolved_path" != "$installed_path" ]]; then
      echo "Warning: your shell will run $resolved_path before $installed_path." >&2
      echo "Run $installed_path directly, remove the older tokenboard, or install to $(dirname "$resolved_path")." >&2
    fi
  elif [[ ":$PATH:" != *":$install_dir:"* ]]; then
    echo "$install_dir is not on PATH. Add it to PATH or run $installed_path directly." >&2
  fi
}

main "$@"
