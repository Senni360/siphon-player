#!/bin/bash
# Ships a Windows x64 release: packaging/release.sh 0.15.0-pre.7
#
# Write packaging/release-notes/vVERSION.md first, and commit everything else.
# The rest is what the updater in people's copies needs: the zip's folder name,
# the portable marker beside siphon.exe, and a signed checksums.txt.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=${1:?usage: packaging/release.sh VERSION}
TAG=v$VERSION
STEM=siphon-$TAG-x86_64-pc-windows-msvc
NOTES=packaging/release-notes/$TAG.md
KEY=${SIPHON_SIGNING_KEY:-$HOME/.siphon-release/update-signing-key.pem}

die() { echo "release: $1" >&2; exit 1; }

test -s "$NOTES" || die "write $NOTES first"
test -f "$KEY" || die "no signing key at $KEY"
test "$(git branch --show-current)" = main || die "not on main"
test -z "$(git status --porcelain | grep -vF "$NOTES" || true)" || die "commit your changes first"
! git rev-parse -q --verify "refs/tags/$TAG" > /dev/null || die "$TAG already exists"

# A wrong key would ship an update every copy refuses, so check before building.
public=$(openssl pkey -in "$KEY" -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \r\n')
test "$public" = "$(tr -d ' \r\n' < assets/update-public-key.hex)" || die "$KEY is not the key the app trusts"

# A failed build leaves nothing behind to clean up by hand.
trap 'git checkout -q -- Cargo.toml Cargo.lock' ERR
sed -i "0,/^version = .*/s//version = \"$VERSION\"/" Cargo.toml
cargo build --release --bin siphon
test "$(target/release/siphon.exe --version | tr -d '\r')" = "siphon $VERSION" || die "wrong version in the binary"
trap - ERR

rm -rf dist
mkdir -p "dist/$STEM"
cp target/release/siphon.exe LICENSE packaging/siphon-portable.txt "dist/$STEM/"
(
    cd dist
    pwsh -NoProfile -Command "Compress-Archive -Path '$STEM' -DestinationPath '$STEM.zip'"
    sha256sum "$STEM.zip" | sed 's/ \*/  /' > checksums.txt
    openssl pkeyutl -sign -rawin -inkey "$KEY" -in checksums.txt -out checksums.txt.sig
    openssl pkeyutl -verify -rawin -inkey "$KEY" -in checksums.txt -sigfile checksums.txt.sig > /dev/null
)

git add Cargo.toml Cargo.lock "$NOTES"
git commit -q -m "Release $TAG"
git push -q origin main

prerelease=()
[[ $VERSION == *-* ]] && prerelease=(--prerelease)
gh release create "$TAG" "dist/$STEM.zip" dist/checksums.txt dist/checksums.txt.sig \
    --target "$(git rev-parse HEAD)" "${prerelease[@]}" \
    --title "Siphon: Encore $VERSION" --notes-file "$NOTES"
rm -rf dist
