#!/usr/bin/env bash
# Copyright (c) 2026 NVIDIA Corporation
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "${script_dir}/../../../.." && pwd)

if [[ $# != 2 ]]; then
	echo "Usage: $0 <static-linux-envd-binary> <new-output-directory>" >&2
	exit 1
fi

binary=$(realpath "$1")
output=$2
[[ -f "${binary}" && -x "${binary}" ]] || {
	echo 'envd must be a regular executable file' >&2
	exit 1
}
# Do not execute the input: cross-architecture extension builds are supported.
header=$(LC_ALL=C readelf -hW "${binary}")
if ! grep -q 'OS/ABI:.*\(UNIX - System V\|UNIX - GNU\)' <<< "${header}" ||
	! grep -q 'Type:.*\(EXEC\|DYN\)' <<< "${header}"; then
	echo 'envd must be a Linux ELF executable' >&2
	exit 1
fi
program_headers=$(LC_ALL=C readelf -lW "${binary}")
dynamic_section=$(LC_ALL=C readelf -dW "${binary}")
if grep -q ' INTERP ' <<< "${program_headers}" ||
	grep -q '(NEEDED)' <<< "${dynamic_section}"; then
	echo 'envd must be statically linked (no interpreter or shared libraries)' >&2
	exit 1
fi

# Refuse an existing output directory to avoid mixing binaries and root hashes.
mkdir "${output}"
output=$(cd "${output}" && pwd)
rootfs="${output}/extension-rootfs"
mkdir -p "${rootfs}/bin"
chmod 0755 "${rootfs}" "${rootfs}/bin"
install -m 0755 "${binary}" "${rootfs}/bin/kata-envd"
(cd "${rootfs}" && sha256sum bin/kata-envd) > "${output}/envd.sha256"

# Use the same partitioned EROFS + dm-verity format as other Kata extensions.
BUILD_VARIANT=envd-extension MEASURED_ROOTFS=yes SKIP_DAX_HEADER=yes \
	SKIP_ROOTFS_CHECK=yes AGENT_INIT=no SELINUX=no \
	bash "${repo_root}/tools/osbuilder/image-builder/image_builder.sh" \
	-f erofs -o "${output}/kata-containers-envd-extension.img" "${rootfs}"

verity_params=$(cat "${output}/root_hash_envd-extension.txt")
cat > "${output}/configuration-envd.toml" <<EOF
# Merge into the chosen hypervisor's configuration. Change qemu as needed.
[[hypervisor.qemu.guest_extension_images]]
name = "kata-envd"
path = "/opt/kata/share/kata-containers/kata-containers-envd-extension.img"
verity_params = "${verity_params}"
EOF

echo "Extension image and runtime configuration: ${output}"
