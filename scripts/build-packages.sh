#!/usr/bin/env bash
# Copyright 2026 Simon Annetts/Jak Daniels
# SPDX-License-Identifier: MPL-2.0

# Builds the Linux packages for this machine's architecture into dist/ (PLAN.md step 6):
#   gstreamer1.0-acousticmultimixer_VERSION_ARCH.deb   (Debian, Ubuntu)
#   gstreamer1-acousticmultimixer-VERSION-1.ARCH.rpm   (Fedora)
# Run it on the oldest system the packages are for (CI: Ubuntu 24.04, GStreamer 1.24, so they need glibc 2.39 or
# later), x64 or arm64. Needs Rust, GStreamer's development files and nfpm (https://nfpm.goreleaser.com; CI pins its
# version). The version comes from the git tags (scripts/version.sh), so run it in a clone with its history; it is
# written into Cargo.toml for the build only (scripts/with-version.sh).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

case "$(uname -m)" in
  x86_64) goarch=amd64 triplet=x86_64-linux-gnu ;;
  aarch64) goarch=arm64 triplet=aarch64-linux-gnu ;;
  *) echo "Unsupported architecture $(uname -m)" >&2; exit 2 ;;
esac

version="$(scripts/version.sh)"
echo "Version $version ($goarch)"

scripts/with-version.sh cargo build --release

stage=dist/stage
rm -rf "$stage"
mkdir -p "$stage" dist
install -m 0644 target/release/libgstacousticmultimixer.so "$stage/"
strip --strip-unneeded "$stage/libgstacousticmultimixer.so"
scripts/changelog.sh "$version" > "$stage/CHANGELOG.md"

for format in deb rpm; do
  # nfpm expands variables in some fields only (not in a file's destination): the multiarch folder is filled in here.
  sed "s|@TRIPLET@|$triplet|g" "packaging/linux/nfpm-$format.yaml" > "$stage/nfpm-$format.yaml"
  VERSION="$version" ARCH="$goarch" nfpm package -f "$stage/nfpm-$format.yaml" -p "$format" -t dist/
done
ls -l dist/*.deb dist/*.rpm
