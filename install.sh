#!/bin/sh
# Instala o Passo a partir das releases do GitHub.
#   curl -fsSL https://raw.githubusercontent.com/JonathanSantos/passo/main/install.sh | sh
# Variáveis: PASSO_VERSION (padrão: a última), PASSO_INSTALL_DIR (padrão: ~/.local/bin)
set -eu

repo="JonathanSantos/passo"
version="${PASSO_VERSION:-latest}"
dir="${PASSO_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  MINGW* | MSYS* | CYGWIN*) echo "passo: no Windows, use o install.ps1 (irm https://raw.githubusercontent.com/$repo/main/install.ps1 | iex)" >&2; exit 1 ;;
  *) echo "passo: sistema não suportado: $(uname -s) (há builds para macOS, Linux e Windows)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) echo "passo: arquitetura não suportada: $(uname -m)" >&2; exit 1 ;;
esac

asset="passo-$arch-$os.tar.gz"
if [ "$version" = "latest" ]; then
  url="https://github.com/$repo/releases/latest/download/$asset"
else
  url="https://github.com/$repo/releases/download/$version/$asset"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "passo: baixando $url"
curl -fsSL "$url" -o "$tmp/$asset"
curl -fsSL "$url.sha256" -o "$tmp/$asset.sha256"
if command -v sha256sum >/dev/null 2>&1; then
  check="sha256sum -c"
else
  check="shasum -a 256 -c"
fi
(cd "$tmp" && $check "$asset.sha256" >/dev/null) || { echo "passo: checksum não confere" >&2; exit 1; }
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dir"
install -m 0755 "$tmp/passo" "$dir/passo"
echo "passo: instalado em $dir/passo"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "passo: adicione $dir ao PATH (ex.: export PATH=\"$dir:\$PATH\")" ;;
esac
