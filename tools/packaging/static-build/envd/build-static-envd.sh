#!/usr/bin/env bash
# Copyright (c) 2026 NVIDIA Corporation
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT
git init "${work}/source"
git -C "${work}/source" remote add origin "${ENVD_URL:?}"
git -C "${work}/source" fetch --depth=1 origin "${ENVD_COMMIT:?}"
git -C "${work}/source" checkout --detach FETCH_HEAD
[[ $(git -C "${work}/source" rev-parse HEAD) == "${ENVD_COMMIT}" ]]

(
	cd "${work}/source/packages/envd"
	GOWORK=off GOTOOLCHAIN=local CGO_ENABLED=0 GOOS=linux go build \
		-trimpath -buildvcs=false -ldflags "-s -w -X main.commitSHA=${ENVD_COMMIT}" \
		-o "${work}/envd" .
)

extension_builder="${REPO_ROOT:?}/tools/packaging/guest-extension/envd"
bash "${extension_builder}/build.sh" "${work}/envd" "${work}/extension"
bash "${extension_builder}/test-image.sh" "${work}/extension"

install_dir="${DESTDIR:?}/opt/kata/share/kata-containers"
mkdir -p "${install_dir}"
for asset in kata-containers-envd-extension.img root_hash_envd-extension.txt configuration-envd.toml envd.sha256; do
	install -m 0644 "${work}/extension/${asset}" "${install_dir}/${asset}"
done
printf '%s\n%s\n' "${ENVD_URL}" "${ENVD_COMMIT}" > "${install_dir}/envd-source.txt"
