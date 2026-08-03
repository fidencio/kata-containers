#!/usr/bin/env bash
#
# Copyright 2022 Intel
#
# SPDX-License-Identifier: Apache-2.0
#

[[ -z "${DEBUG}" ]] || set -x
set -o errexit
set -o nounset
set -o pipefail
set -o errtrace

SCRIPT_DIR="$(cd "$(dirname "${0}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

REGISTRY="${1:-"quay.io/kata-containers/kata-deploy"}"
TAG="${2:-}"
ARTIFACTS_BUILD_DIR="${3:-${REPO_ROOT}/tools/packaging/kata-deploy/local-build/build}"
# Separate, minimal image for the job-mode dispatcher (kata-deploy-job-dispatcher).
# Built from its own staged tarball, with the same tag scheme as the kata-deploy
# image. The repo name mirrors the kata-deploy repo with "-job-dispatcher" inserted
# before any "-ci" suffix, so the "-ci" stays last:
#   .../kata-deploy     -> .../kata-deploy-job-dispatcher
#   .../kata-deploy-ci  -> .../kata-deploy-job-dispatcher-ci
if [[ "${REGISTRY}" == *-ci ]]; then
	default_job_dispatcher_image_reference="${REGISTRY%-ci}-job-dispatcher-ci"
else
	default_job_dispatcher_image_reference="${REGISTRY}-job-dispatcher"
fi
JOB_DISPATCHER_IMAGE_REFERENCE="${4:-${default_job_dispatcher_image_reference}}"

# When set, the images are written to this directory as docker archives instead
# of being pushed. It serves callers that have no registry to push to - notably
# the pull_request CI lane, which runs a fork's code with a read-only token -
# and which side-load the archives into their test cluster instead.
IMAGE_TARBALL_DIR="${KATA_IMAGE_TARBALL_DIR:-}"

KATA_DEPLOY_DIR="${REPO_ROOT}/tools/packaging/kata-deploy"
ARTIFACTS_STAGE_DIR="${KATA_DEPLOY_DIR}/kata-artifacts"

# Stage the component tarballs into a directory that is visible to the
# Docker build context (local-build/ is excluded via .dockerignore).
mkdir -p "${ARTIFACTS_STAGE_DIR}"
cp "${ARTIFACTS_BUILD_DIR}"/kata-static-*.tar.zst "${ARTIFACTS_STAGE_DIR}/"
cp "${ARTIFACTS_BUILD_DIR}"/kata-deploy-static-*.tar.zst "${ARTIFACTS_STAGE_DIR}/"

cleanup() {
	rm -rf "${ARTIFACTS_STAGE_DIR}"
}
trap cleanup EXIT

pushd "${REPO_ROOT}"

arch=$(uname -m)
[[ "${arch}" = "x86_64" ]] && arch="amd64"
[[ "${arch}" = "aarch64" ]] && arch="arm64"
PLATFORM="linux/${arch}"
COMMIT_TAG="kata-containers-$(git -C "${REPO_ROOT}" rev-parse HEAD)-${arch}"
IMAGE_TAG="${REGISTRY}:${COMMIT_TAG}"
JOB_DISPATCHER_IMAGE_TAG="${JOB_DISPATCHER_IMAGE_REFERENCE}:${COMMIT_TAG}"

DOCKERFILE="${REPO_ROOT}/tools/packaging/kata-deploy/Dockerfile"
JOB_DISPATCHER_DOCKERFILE="${REPO_ROOT}/tools/packaging/kata-deploy/job-dispatcher/Dockerfile"

# Build one image under every tag it was given, and either push it or write it
# to a docker archive. One build covers all the tags either way: buildx pushes
# each of them, and a docker archive carries a RepoTags list that
# `ctr images import` registers in full.
build_image() {
	local dockerfile="${1}"
	local archive="${2}"
	shift 2

	local build_args=()
	local tag
	for tag in "${@}"; do
		build_args+=(--tag "${tag}")
	done

	if [[ -n "${IMAGE_TARBALL_DIR}" ]]; then
		build_args+=(--output "type=docker,dest=${IMAGE_TARBALL_DIR}/${archive}")
	else
		build_args+=(--push)
	fi

	# Disable provenance and SBOM so each tag is a single image manifest. quay.io rejects
	# pushing multi-arch manifest lists that include attestation manifests ("manifest invalid").
	docker buildx build --platform "${PLATFORM}" --provenance false --sbom false \
		-f "${dockerfile}" "${build_args[@]}" .
}

kata_deploy_tags=("${IMAGE_TAG}")
job_dispatcher_tags=("${JOB_DISPATCHER_IMAGE_TAG}")
if [[ -n "${TAG}" ]]; then
	kata_deploy_tags+=("${REGISTRY}:${TAG}")
	job_dispatcher_tags+=("${JOB_DISPATCHER_IMAGE_REFERENCE}:${TAG}")
fi

[[ -z "${IMAGE_TARBALL_DIR}" ]] || mkdir -p "${IMAGE_TARBALL_DIR}"

echo "Building the kata-deploy image: ${kata_deploy_tags[*]}"
build_image "${DOCKERFILE}" "kata-deploy.tar" "${kata_deploy_tags[@]}"

echo "Building the kata-deploy-job-dispatcher image: ${job_dispatcher_tags[*]}"
build_image "${JOB_DISPATCHER_DOCKERFILE}" "kata-deploy-job-dispatcher.tar" "${job_dispatcher_tags[@]}"

popd
