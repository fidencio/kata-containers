#!/usr/bin/env bash
# Copyright (c) 2026 NVIDIA Corporation
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=/dev/null
source "${script_dir}/../../scripts/lib.sh"

case "${ARCH}" in
	x86_64|aarch64) ;;
	*) die "envd extension is unsupported on ${ARCH}" ;;
esac
container_image=$(get_envd_image_name)
docker build --build-arg GO_TOOLCHAIN="$(get_from_kata_deps '.externals.envd.toolchain')" \
	-t "${container_image}" "${script_dir}"

mkdir -p "${DESTDIR:?}"
docker run --rm \
	-v "${repo_root_dir}:${repo_root_dir}:ro" \
	-v "${DESTDIR}:${DESTDIR}" \
	--user "$(id -u):$(id -g)" \
	--env HOME=/tmp \
	--env ENVD_URL="$(get_from_kata_deps '.externals.envd.url')" \
	--env ENVD_COMMIT="$(get_from_kata_deps '.externals.envd.commit')" \
	--env GOARCH="$(arch_to_golang "${ARCH}")" \
	--env DESTDIR \
	--env REPO_ROOT="${repo_root_dir}" \
	"${container_image}" bash "${script_dir}/build-static-envd.sh"
