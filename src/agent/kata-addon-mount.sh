#!/bin/bash
#
# Copyright (c) 2026 NVIDIA Corporation
#
# SPDX-License-Identifier: Apache-2.0
#
# Mount a kata addon block device with optional dm-verity verification.
# Usage: kata-addon-mount.sh <addon-name>
#
# The block device is discovered by scanning /sys/block/*/serial for
# a device whose serial matches "addon-<name>".
# Verity params are read from kernel cmdline: kata.addon.<name>.verity_params=...
# The addon is mounted read-only (erofs) at /run/kata-addons/<name>/.

set -euo pipefail

ADDON_NAME="${1:?addon name required}"
SERIAL="addon-${ADDON_NAME}"
MOUNT_DIR="/run/kata-addons/${ADDON_NAME}"

find_block_dev_by_serial() {
	local wanted="$1"
	for s in /sys/block/*/serial; do
		[[ -f "${s}" ]] || continue
		local cur
		cur="$(cat "${s}" 2>/dev/null)" || continue
		if [[ "${cur}" == "${wanted}" ]]; then
			echo "/dev/$(basename "$(dirname "${s}")")"
			return 0
		fi
	done
	return 1
}

REAL_DEV="$(find_block_dev_by_serial "${SERIAL}")" || {
	echo "ERROR: no block device with serial ${SERIAL} found" >&2
	exit 1
}

get_verity_param() {
	local key="kata.addon.${ADDON_NAME}.verity_params"
	local cmdline
	cmdline="$(cat /proc/cmdline)"

	local value=""
	for param in ${cmdline}; do
		case "${param}" in
			"${key}="*)
				value="${param#"${key}="}"
				;;
		esac
	done
	echo "${value}"
}

parse_verity_field() {
	local params="$1"
	local field="$2"
	echo "${params}" | tr ',' '\n' | while IFS='=' read -r k v; do
		if [[ "${k}" == "${field}" ]]; then
			echo "${v}"
			return
		fi
	done
}

VERITY_PARAMS="$(get_verity_param)"

if [[ -n "${VERITY_PARAMS}" ]]; then
	ROOT_HASH="$(parse_verity_field "${VERITY_PARAMS}" "root_hash")"
	SALT="$(parse_verity_field "${VERITY_PARAMS}" "salt")"
	DATA_BLOCKS="$(parse_verity_field "${VERITY_PARAMS}" "data_blocks")"
	HASH_BLOCK_SIZE="$(parse_verity_field "${VERITY_PARAMS}" "hash_block_size")"
	DATA_BLOCK_SIZE="$(parse_verity_field "${VERITY_PARAMS}" "data_block_size")"

	DATA_DEV="${REAL_DEV}p1"
	HASH_DEV="${REAL_DEV}p2"

	DM_NAME="addon-${ADDON_NAME}"

	veritysetup open "${DM_NAME}" "${DATA_DEV}" "${HASH_DEV}" "${ROOT_HASH}" \
		--hash-block-size="${HASH_BLOCK_SIZE:-4096}" \
		--data-block-size="${DATA_BLOCK_SIZE:-4096}" \
		--data-blocks="${DATA_BLOCKS}" \
		--salt="${SALT}"

	MOUNT_SRC="/dev/mapper/${DM_NAME}"
else
	if [[ -e "${REAL_DEV}p1" ]]; then
		MOUNT_SRC="${REAL_DEV}p1"
	else
		MOUNT_SRC="${REAL_DEV}"
	fi
fi

mkdir -p "${MOUNT_DIR}"
mount -t erofs -o ro "${MOUNT_SRC}" "${MOUNT_DIR}"

# Bind-mount addon binaries into /usr/local/bin/ so the agent and
# CoCo services can find them in the standard PATH.
if [[ -d "${MOUNT_DIR}/usr/local/bin" ]]; then
	for bin in "${MOUNT_DIR}"/usr/local/bin/*; do
		[[ -f "${bin}" ]] || continue
		target="/usr/local/bin/$(basename "${bin}")"
		touch "${target}" 2>/dev/null || true
		mount --bind "${bin}" "${target}"
	done
fi

# Bind-mount addon config files into /etc/.
if [[ -d "${MOUNT_DIR}/etc" ]]; then
	for cfg in "${MOUNT_DIR}"/etc/*; do
		[[ -e "${cfg}" ]] || continue
		target="/etc/$(basename "${cfg}")"
		if [[ -f "${cfg}" ]]; then
			touch "${target}" 2>/dev/null || true
			mount --bind "${cfg}" "${target}"
		elif [[ -d "${cfg}" ]]; then
			mkdir -p "${target}"
			mount --bind "${cfg}" "${target}"
		fi
	done
fi

# Bind-mount pause_bundle if present.
if [[ -d "${MOUNT_DIR}/pause_bundle" ]]; then
	mkdir -p /pause_bundle
	mount --bind "${MOUNT_DIR}/pause_bundle" /pause_bundle
fi

echo "Addon ${ADDON_NAME} mounted at ${MOUNT_DIR}"
