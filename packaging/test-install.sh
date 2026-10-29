#!/usr/bin/env bash
# Usage: bash packaging/test-install.sh ubuntu:24.04 dist/native-packages
# Run on the matching architecture, with a C compiler and Docker available.
set -euo pipefail

image=${1:?Supply a Debian, Ubuntu or Fedora container image}
packages=$(realpath "${2:?Supply a native-packages output directory}")
case "$(uname -m)" in
  x86_64) target=linux-amd64 ;;
  aarch64) target=linux-arm64 ;;
  *) echo 'Unsupported test architecture' >&2; exit 1 ;;
esac
case "$image" in
  ubuntu:*|debian:*) format=deb ;;
  fedora:*) format=rpm ;;
  *) echo 'Unsupported test distribution' >&2; exit 1 ;;
esac
package_dir="$packages/packages/$target/$format"
test -d "$package_dir"
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
checks=$(mktemp -d)
trap 'rm -rf -- "$checks"' EXIT
cc -std=c99 -Wall -Wextra -Werror "$script_dir/check-runtime-libs.c" -ldl -o "$checks/check-runtime-libs"

docker run --rm \
  --volume "$package_dir:/packages:ro" \
  --volume "$checks:/checks:ro" \
  --env "FORMAT=$format" \
  "$image" sh -ec '
    set -- /packages/*."$FORMAT"
    test "$#" -eq 1
    test -f "$1"
    mkdir -p /root/.config/siphon
    printf "%s\n" "preserve-existing-settings" > /root/.config/siphon/settings-fixture
    if [ "$FORMAT" = deb ]; then
      apt-get update
      DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$1"
      dpkg-query -W siphon
    else
      dnf install -y --setopt=install_weak_deps=False "$1"
      rpm -q siphon
    fi
    # --version exercises linked libraries; the probe checks dlopen libraries
    # without installing a desktop, compiler, interpreter or test dependencies.
    # Trace the isolated fixture assertions so a failed check identifies itself.
    set -x
    siphon --version
    test -f /usr/bin/siphon
    test ! -L /usr/bin/siphon
    test -f /usr/share/licenses/siphon/LICENSE
    if [ "$FORMAT" = deb ]; then
      # Slim Debian/Ubuntu images exclude /usr/share/doc at installation time.
      # Verify the regular file in the package, not the intentionally stripped root.
      dpkg-deb --contents "$1" | grep -E "^-.* ./usr/share/doc/siphon/README.md$"
    else
      test -f /usr/share/doc/siphon/README.md
    fi
    /checks/check-runtime-libs
    test -s /usr/share/applications/siphon.desktop
    test -s /usr/share/icons/hicolor/scalable/apps/siphon.svg
    grep -qx "Icon=siphon" /usr/share/applications/siphon.desktop
    grep -qx "StartupWMClass=siphon" /usr/share/applications/siphon.desktop
    test -s /usr/share/siphon/omarchy/siphon.json.tpl
    test -x /usr/share/siphon/omarchy/siphon-theme
    test "$(cat /root/.config/siphon/settings-fixture)" = preserve-existing-settings
    if [ "$FORMAT" = deb ]; then
      apt-get remove -y siphon
    else
      dnf remove -y siphon
    fi
    test ! -e /usr/bin/siphon
    test ! -e /usr/share/applications/siphon.desktop
    test ! -e /usr/share/icons/hicolor/scalable/apps/siphon.svg
    test ! -e /usr/share/siphon/omarchy/siphon.json.tpl
    test ! -e /usr/share/siphon/omarchy/siphon-theme
    test "$(cat /root/.config/siphon/settings-fixture)" = preserve-existing-settings
  '
