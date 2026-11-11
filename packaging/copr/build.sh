#!/usr/bin/env bash
# Build Irori's COPR source package from the Linux tarballs, rebuild it the
# way a COPR chroot does, and install the result the way users will. Runs as
# root in a Fedora container.
#
#   packaging/copr/build.sh <version> <dist dir with both tarballs> <output dir> <owner/repo>
set -euo pipefail

version=$1
dist=$(realpath "$2")
mkdir -p "$3"
out=$(realpath "$3")
repo=$4
root=$(cd "$(dirname "$0")/../.." && pwd)

dnf install -y --setopt=install_weak_deps=False python3 rpm-build

work=$(mktemp -d)
python3 "$root/packaging/release_manifests.py" --version "$version" --dist "$dist" \
  --repo "$repo" --output "$work" --copr
# What publish.yml sends to COPR: the spec with both tarballs inside.
rpmbuild -bs --define "_sourcedir $dist" --define "_srcrpmdir $out" "$work/copr/irori.spec"
srpm=$(echo "$out"/irori-"$version"-*.src.rpm)
rpm -qlp "$srpm" | grep -qx "Irori-$version-linux.tar.gz"
rpm -qlp "$srpm" | grep -qx "Irori-$version-linux-arm64.tar.gz"
rpmbuild --rebuild --define "_rpmdir $work/rpms" "$srpm"
arch=$(rpm --eval '%{_arch}')

# Irori alone first: it must start without the sign-in window's libraries.
dnf install -y --setopt=install_weak_deps=False "$work"/rpms/"$arch"/irori-"$version"-*."$arch".rpm
irori --version
dnf install -y "$work"/rpms/"$arch"/irori-sign-in-"$version"-*."$arch".rpm
set +e
irori-sign-in --unsupported 2> "$work/error.txt"
status=$?
set -e
cat "$work/error.txt"
[ "$status" -eq 1 ] && grep -q 'unknown argument --unsupported' "$work/error.txt"
ls -l "$out"
