#!/usr/bin/env bash
# Copyright (c) 2026 NVIDIA Corporation
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

output=$(realpath "${1:?usage: test-image.sh <build-output-directory>}")
image="${output}/kata-containers-envd-extension.img"
work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT

for part in 1 2; do
	read -r start size < <(sfdisk --json "${image}" |
		jq -r --argjson part "${part}" \
		'.partitiontable.partitions[$part - 1] | "\(.start) \(.size)"')
	dd if="${image}" of="${work}/p${part}" bs=512 skip="${start}" count="${size}" status=none
done

fsck.erofs --extract="${work}/extracted" "${work}/p1"
cmp "${output}/extension-rootfs/bin/kata-envd" "${work}/extracted/bin/kata-envd"
[[ -x "${work}/extracted/bin/kata-envd" ]]
(cd "${work}/extracted" && sha256sum -c "${output}/envd.sha256")

IFS=, read -r -a fields < "${output}/root_hash_envd-extension.txt"
options=(--no-superblock)
root_hash=
for field in "${fields[@]}"; do
	key=${field%%=*}
	value=${field#*=}
	case "${key}" in
		root_hash) root_hash=${value} ;;
		salt|data_blocks|hash_block_size|data_block_size)
			options+=("--${key//_/-}=${value}") ;;
		*) echo "Unexpected verity parameter: ${key}" >&2; exit 1 ;;
	esac
done
[[ -n "${root_hash}" ]]
veritysetup verify "${work}/p1" "${work}/p2" "${root_hash}" "${options[@]}"
# Corruption in the data partition must be detected by the same parameters.
printf '\377' | dd of="${work}/p1" bs=1 seek=0 conv=notrunc status=none
if veritysetup verify "${work}/p1" "${work}/p2" "${root_hash}" "${options[@]}" \
	> "${work}/tampered.log" 2>&1; then
	echo 'dm-verity accepted a corrupted data partition' >&2
	exit 1
fi

grep -Fxq 'name = "kata-envd"' "${output}/configuration-envd.toml"
grep -Fxq "verity_params = \"$(cat "${output}/root_hash_envd-extension.txt")\"" \
	"${output}/configuration-envd.toml"
echo 'Extension payload and dm-verity checks passed'
