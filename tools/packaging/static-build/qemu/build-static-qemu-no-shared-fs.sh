#!/usr/bin/env bash
#
# Copyright (c) 2026 NVIDIA Corporation
#
# SPDX-License-Identifier: Apache-2.0
#
# Builds the same QEMU as build-static-qemu.sh, but with the device set
# trimmed down to what the runtime classes that run without a shared
# filesystem emit.  The binaries carry a "no-shared-fs" suffix, so this
# tarball and the full one can be installed side by side.

set -o errexit
set -o nounset
set -o pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# shellcheck source=/dev/null
source "${script_dir}/../../scripts/lib.sh"

qemu_repo="${qemu_repo:-}"
qemu_version="${qemu_version:-}"

if [[ -z "${qemu_repo}" ]]; then
	info "Get qemu information from runtime versions.yaml"
	qemu_url=$(get_from_kata_deps ".assets.hypervisor.qemu.url")
	[[ -n "${qemu_url}" ]] || die "failed to get qemu url"
	qemu_repo="${qemu_url}.git"
fi
[[ -n "${qemu_repo}" ]] || die "failed to get qemu repo"

[[ -n "${qemu_version}" ]] || qemu_version=$(get_from_kata_deps ".assets.hypervisor.qemu.version")
[[ -n "${qemu_version}" ]] || die "failed to get qemu version"

"${script_dir}/build-base-qemu.sh" "${qemu_repo}" "${qemu_version}" "no-shared-fs" "kata-static-qemu-no-shared-fs.tar.gz"
