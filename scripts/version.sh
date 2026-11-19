#!/usr/bin/env bash
# Copyright 2026 Simon Annetts/Jak Daniels
# SPDX-License-Identifier: MPL-2.0

# Prints this build's version (SemVer) from the git tags, as MinVer does for SpatialAudioServer:
#   on a release tag vX.Y.Z               X.Y.Z
#   N commits after vX.Y.Z                X.Y.(Z+1)-alpha.0.N   (a draft: older than the next release)
#   N commits after a pre-release tag     its version .N        (v0.2.0-rc.1 → 0.2.0-rc.1.N)
#   no release tag yet                    0.1.0-alpha.0.N       (N: every commit)
# Needs the history and the tags (fetch-depth: 0).
set -euo pipefail

tag="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null)" || tag=""
if [ -z "$tag" ]; then
  echo "0.1.0-alpha.0.$(git rev-list --count HEAD)"
  exit 0
fi
version="${tag#v}"
n="$(git rev-list --count "$tag..HEAD")"
if [ "$n" = 0 ]; then
  echo "$version"
elif [ "$version" != "${version%%-*}" ]; then
  echo "$version.$n"
else
  IFS=. read -r major minor patch <<< "$version"
  echo "$major.$minor.$((patch + 1))-alpha.0.$n"
fi
