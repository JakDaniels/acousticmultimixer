#!/bin/sh
# Copyright 2026 Simon Annetts/Jak Daniels
# SPDX-License-Identifier: MPL-2.0

# Installs the package in this (clean, root) system and checks it works: GStreamer finds the element, with the
# package's version, and a short mix of two voices in a room runs through it. Used by .github/workflows/packages.yml
# on each distribution.
#   scripts/test-packages.sh DIR
# DIR holds the packages (.deb or .rpm, whichever this system uses) for this machine's architecture.
set -eu
dir="$(cd "$1" && pwd)" # absolute: apt takes "packages/x.deb" for a package name, "/…/x.deb" for a file

case "$(uname -m)" in
  x86_64) deb=amd64 rpm=x86_64 ;;
  aarch64) deb=arm64 rpm=aarch64 ;;
  *) echo "Unsupported architecture $(uname -m)" >&2; exit 2 ;;
esac
if command -v apt-get >/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -q
  package=$(ls "$dir"/gstreamer1.0-acousticmultimixer_*_"$deb".deb)
  apt-get install -y -q "$package" gstreamer1.0-tools gstreamer1.0-plugins-base
elif command -v dnf >/dev/null; then
  package=$(ls "$dir"/gstreamer1-acousticmultimixer-[0-9]*."$rpm".rpm)
  dnf install -y -q "$package" gstreamer1
else
  echo "Neither apt-get nor dnf here." >&2
  exit 2
fi

echo "== the element"
gst-inspect-1.0 acousticmultimixer > inspect.txt
head -n 12 inspect.txt
# The package's version (0.1.1~alpha.0.5) is the plugin's (0.1.1-alpha.0.5-<commit>).
case "$package" in
  *.deb) version=$(dpkg-deb -f "$package" Version) ;;
  *) version=$(rpm -qp --qf '%{VERSION}' "$package") ;;
esac
version=$(echo "$version" | tr '~' '-')
grep -q "Version *$version-" inspect.txt || { echo "Expected version $version" >&2; exit 1; }

echo "== a mix"
# A queue before each sink: the element pushes every output from one thread, and a sink waiting to preroll would
# hold it before the others had a buffer.
voice="audio/x-raw,format=F32LE,rate=48000,channels=1"
timeout 60 gst-launch-1.0 -q acousticmultimixer name=m room=cathedral sink_1::x=4 \
  audiotestsrc num-buffers=200 ! "$voice" ! m.sink_0 \
  audiotestsrc num-buffers=200 freq=660 ! "$voice" ! m.sink_1 \
  m.mix_0 ! queue ! fakesink m.mix_1 ! queue ! fakesink
echo "PASS"
