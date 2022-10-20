#!/usr/bin/env bash
#
# Copyright (c) 2021 Intel Corporation
#
# SPDX-License-Identifier: Apache-2.0

set -o errexit
set -o nounset
set -o pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly repo_root_dir="$(cd "${script_dir}/../../../.." && pwd)"
readonly kernel_builder="${repo_root_dir}/tools/packaging/kernel/build-kernel.sh"

source "${script_dir}/../../scripts/lib.sh"

GO_VERSION=${GO_VERSION}
RUST_VERSION=${RUST_VERSION}

DESTDIR=${DESTDIR:-${PWD}}
PREFIX=${PREFIX:-/opt/kata}
container_image="${CC_BUILDER_REGISTRY}:shim-v2-${GO_VERSION}-${RUST_VERSION}-$(get_last_modification ${script_dir}/Dockerfile)"

EXTRA_OPTS="${EXTRA_OPTS:-""}"
REMOVE_VMM_CONFIGS="${REMOVE_VMM_CONFIGS:-""}"

sudo docker pull ${container_image} || \
	(sudo docker build \
		--build-arg GO_VERSION="${GO_VERSION}" \
	      	--build-arg RUST_VERSION="${RUST_VERSION}" \
		-t "${container_image}" "${script_dir}" && \
	 # No-op unless PUSH_TO_REGISTRY is exported as "yes"
	 push_to_registry "${container_image}")

arch=$(uname -m)
if [ ${arch} = "ppc64le" ]; then
	arch="ppc64"
fi

sudo docker run --rm -i -v "${repo_root_dir}:${repo_root_dir}" \
	-w "${repo_root_dir}/src/runtime-rs" \
	"${container_image}" \
	bash -c "git config --global --add safe.directory ${repo_root_dir} && make PREFIX=${PREFIX} QEMUCMD=qemu-system-${arch}"

sudo docker run --rm -i -v "${repo_root_dir}:${repo_root_dir}" \
	-w "${repo_root_dir}/src/runtime-rs" \
	"${container_image}" \
	bash -c "git config --global --add safe.directory ${repo_root_dir} && make PREFIX="${PREFIX}" DESTDIR="${DESTDIR}" install"
	
sudo docker run --rm -i -v "${repo_root_dir}:${repo_root_dir}" \
	-w "${repo_root_dir}/src/runtime" \
	"${container_image}" \
	bash -c "git config --global --add safe.directory ${repo_root_dir} && make PREFIX=${PREFIX} QEMUCMD=qemu-system-${arch} ${EXTRA_OPTS}"

sudo docker run --rm -i -v "${repo_root_dir}:${repo_root_dir}" \
	-w "${repo_root_dir}/src/runtime" \
	"${container_image}" \
	bash -c "git config --global --add safe.directory ${repo_root_dir} && make PREFIX="${PREFIX}" DESTDIR="${DESTDIR}"  ${EXTRA_OPTS} install"

sudo sed -i -e '/^initrd =/d' "${DESTDIR}/${PREFIX}/share/defaults/kata-containers/configuration-qemu.toml"
sudo sed -i -e '/^initrd =/d' "${DESTDIR}/${PREFIX}/share/defaults/kata-containers/configuration-fc.toml"

for vmm in ${REMOVE_VMM_CONFIGS}; do
	sudo rm "${DESTDIR}/${PREFIX}/share/defaults/kata-containers/configuration-$vmm.toml"
done

pushd "${DESTDIR}/${PREFIX}/share/defaults/kata-containers"
	sudo ln -sf "configuration-qemu.toml" configuration.toml
popd
