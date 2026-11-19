#!/usr/bin/env bash
# Copyright 2026 Simon Annetts/Jak Daniels
# SPDX-License-Identifier: MPL-2.0

# Runs a command (a build) with this build's version (scripts/version.sh) in Cargo.toml and Cargo.lock, then puts
# both back: gst-inspect-1.0 shows the version, and git history stays as it is.
#   scripts/with-version.sh cargo build --release
# Git is told to overlook the edit meanwhile, so a clean checkout still builds as clean (the version helper marks
# the commit with "+" when files are modified). Used by build-packages.sh and packages.yml (Windows).
set -euo pipefail
cd "$(dirname "$0")/.."

version="$(scripts/version.sh)"
cp Cargo.toml Cargo.toml.orig
cp Cargo.lock Cargo.lock.orig
restore() {
  mv Cargo.toml.orig Cargo.toml
  mv Cargo.lock.orig Cargo.lock
  git update-index --no-assume-unchanged Cargo.toml Cargo.lock
}
trap restore EXIT
git update-index --assume-unchanged Cargo.toml Cargo.lock
sed -i "0,/^version = \".*\"/s//version = \"$version\"/" Cargo.toml
cargo update --offline -p gst-plugin-acousticmultimixer >/dev/null 2>&1 || cargo update -p gst-plugin-acousticmultimixer
echo "Version $version"
"$@"
