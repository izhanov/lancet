#!/bin/sh
# Lancet installer: downloads the latest prebuilt `lt` from GitHub Releases.
#
#   curl -fsSL https://raw.githubusercontent.com/izhanov/lancet/main/install.sh | sh
#
# With gh installed and logged in, the release is downloaded through it
# (authenticated, so no anonymous API rate limit); otherwise with curl.
# Override the install directory with LANCET_INSTALL_DIR (default ~/.local/bin).
set -eu

REPO="izhanov/lancet"
INSTALL_DIR="${LANCET_INSTALL_DIR:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)

case "$os" in
Darwin) os=darwin ;;
Linux) os=linux ;;
*)
	echo "unsupported OS: $os (build from source: https://github.com/$REPO)" >&2
	exit 1
	;;
esac

case "$arch" in
x86_64 | amd64) arch=amd64 ;;
arm64 | aarch64) arch=arm64 ;;
*)
	echo "unsupported architecture: $arch (build from source: https://github.com/$REPO)" >&2
	exit 1
	;;
esac

asset="lancet_${os}_${arch}.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
	tag=$(gh release view --repo "$REPO" --json tagName --jq .tagName)
	echo "downloading lt $tag (${os}/${arch}) via gh..."
	gh release download "$tag" --repo "$REPO" --pattern "$asset" --dir "$tmp"
else
	tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
		sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1) || true
	if [ -z "$tag" ]; then
		echo "cannot determine the latest release of $REPO" >&2
		exit 1
	fi
	echo "downloading lt $tag (${os}/${arch})..."
	curl -fSL --progress-bar "https://github.com/$REPO/releases/download/$tag/$asset" -o "$tmp/$asset"
fi
tar -xzf "$tmp/$asset" -C "$tmp"

mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/lt" "$INSTALL_DIR/lt"
echo "installed: $INSTALL_DIR/lt ($("$INSTALL_DIR/lt" --version))"

case ":$PATH:" in
*":$INSTALL_DIR:"*) ;;
*)
	echo
	echo "note: $INSTALL_DIR is not in your PATH; add to your shell profile:"
	echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
	;;
esac

echo
echo "next:"
command -v ollama >/dev/null 2>&1 || echo "  install Ollama: https://ollama.com/download"
command -v pdftotext >/dev/null 2>&1 || echo "  for PDFs install Poppler: brew install poppler (or apt install poppler-utils)"
echo "  ollama pull nomic-embed-text   # bge-m3 for notes in other languages"
echo "  lt index ~/notes"
if [ -d "$HOME/.claude" ] && [ ! -f "$HOME/.claude/skills/lancet-search/SKILL.md" ]; then
	echo "  lt skill install               # teaches Claude Code agents to search with lt"
fi
