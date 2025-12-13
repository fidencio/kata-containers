#!/usr/bin/env bash
# Copyright (c) 2025 Kata Containers Community
# Copyright (c) 2025 NVIDIA Corporation
#
# SPDX-License-Identifier: Apache-2.0
#
# Central CI script for GitHub Actions workflows.
# This script contains logic that would otherwise be embedded in workflow YAML files,
# making it testable from PRs even when workflows themselves cannot be modified.

set -o errexit
set -o nounset
set -o pipefail

DEBUG="${DEBUG:-}"
[[ -n "${DEBUG}" ]] && set -x

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root_dir="$(cd "${script_dir}/.." && pwd)"

# shellcheck source=/dev/null
source "${repo_root_dir}/tests/common.bash"

function info() {
	echo "[INFO] $*"
}

function die() {
	echo "[ERROR] $*" >&2
	exit 1
}

#
# check-workflow-trigger: Determine if we should proceed with publishing
# Inputs (via environment):
#   WORKFLOW_CONCLUSION - conclusion of triggering workflow
#   WORKFLOW_EVENT - event type of triggering workflow
#   PR_NUMBER - pull request number
#   HEAD_SHA - head commit SHA
#   RUN_ID - workflow run ID
# Outputs (to GITHUB_OUTPUT if set):
#   should-publish, pr-number, head-sha, run-id
#
function check_workflow_trigger() {
	info "Workflow conclusion: ${WORKFLOW_CONCLUSION:-}"
	info "Workflow event: ${WORKFLOW_EVENT:-}"
	info "PR number: ${PR_NUMBER:-}"
	info "Head SHA: ${HEAD_SHA:-}"

	local should_publish="false"
	if [[ "${WORKFLOW_CONCLUSION:-}" == "success" && \
	      "${WORKFLOW_EVENT:-}" == "pull_request" && \
	      -n "${PR_NUMBER:-}" ]]; then
		should_publish="true"
	fi

	if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
		if [[ "${should_publish}" == "true" ]]; then
			{
				echo "should-publish=true"
				echo "pr-number=${PR_NUMBER}"
				echo "head-sha=${HEAD_SHA}"
				echo "run-id=${RUN_ID:-}"
			} >> "${GITHUB_OUTPUT}"
		else
			echo "should-publish=false" >> "${GITHUB_OUTPUT}"
		fi
	fi

	echo "${should_publish}"
}

#
# trigger-dispatch: Send a repository_dispatch event
# Inputs (via environment):
#   GH_TOKEN - GitHub token
#   GITHUB_REPOSITORY - owner/repo
#   EVENT_TYPE - dispatch event type
#   PR_NUMBER - pull request number
#   HEAD_SHA - head commit SHA
#   TAG - image tag
#
function trigger_dispatch() {
	local event_type="${EVENT_TYPE:-}"
	[[ -z "${event_type}" ]] && die "EVENT_TYPE is required"
	[[ -z "${GITHUB_REPOSITORY:-}" ]] && die "GITHUB_REPOSITORY is required"
	[[ -z "${PR_NUMBER:-}" ]] && die "PR_NUMBER is required"
	[[ -z "${HEAD_SHA:-}" ]] && die "HEAD_SHA is required"
	[[ -z "${TAG:-}" ]] && die "TAG is required"

	info "Triggering ${event_type} workflow"
	gh api "repos/${GITHUB_REPOSITORY}/dispatches" \
		--method POST \
		-f "event_type=${event_type}" \
		-f "client_payload[pr_number]=${PR_NUMBER}" \
		-f "client_payload[head_sha]=${HEAD_SHA}" \
		-f "client_payload[tag]=${TAG}"
	info "${event_type} workflow triggered"
}

#
# build-asset: Build a kata component tarball
# Inputs (via environment):
#   KATA_ASSET - asset to build
#   PUSH_TO_REGISTRY - whether to push (yes/no)
#   RELEASE - whether this is a release build (yes/no)
#   CI_HKD_PATH - (optional) path for s390x HKD
#   KBUILD_SIGN_PIN - (optional) kernel signing PIN
#
function build_asset() {
	local asset="${KATA_ASSET:-}"
	[[ -z "${asset}" ]] && die "KATA_ASSET is required"

	info "Building ${asset}"
	make "${asset}-tarball"

	local build_dir
	build_dir=$(readlink -f build)
	mkdir -p kata-build
	cp "${build_dir}"/kata-static-"${asset}"*.tar.* kata-build/.
	info "Built ${asset} successfully"
}

#
# run-protected-tests: Run tests on protected infrastructure
# Inputs (via environment):
#   TEST_TYPE - type of test (aks, arm64, coco, nvidia-gpu)
#   DOCKER_REGISTRY, DOCKER_REPO, DOCKER_TAG - container image info
#   KATA_HYPERVISOR - (optional) hypervisor to use
#   KBS - (optional) whether to use KBS
#
function run_protected_tests() {
	local test_type="${TEST_TYPE:-}"
	[[ -z "${test_type}" ]] && die "TEST_TYPE is required"

	info "Running ${test_type} tests"
	info "Registry: ${DOCKER_REGISTRY:-}/${DOCKER_REPO:-}:${DOCKER_TAG:-}"
	info "Hypervisor: ${KATA_HYPERVISOR:-default}"
	info "KBS: ${KBS:-false}"

	case "${test_type}" in
		aks)
			# Call existing AKS test script when ready
			info "AKS tests would run here"
			# "${repo_root_dir}/tests/integration/kubernetes/gha-run.sh" run
			;;
		arm64)
			# Call existing arm64 K8s test script when ready
			info "arm64 K8s tests would run here"
			# "${repo_root_dir}/tests/integration/kubernetes/gha-run.sh" run
			;;
		coco)
			# Call existing CoCo test script when ready
			info "CoCo tests would run here"
			# "${repo_root_dir}/tests/integration/coco/gha-run.sh" run
			;;
		nvidia-gpu)
			# Call existing NVIDIA GPU test script when ready
			info "NVIDIA GPU tests would run here"
			# "${repo_root_dir}/tests/integration/kubernetes/gha-run.sh" run
			;;
		*)
			die "Unknown test type: ${test_type}"
			;;
	esac

	info "${test_type} tests completed"
}

function main() {
	local action="${1:-}"

	case "${action}" in
		check-workflow-trigger)
			check_workflow_trigger
			;;
		trigger-protected-tests)
			EVENT_TYPE="new-ci-protected-tests" trigger_dispatch
			;;
		trigger-arch-build)
			local arch="${2:-}"
			[[ -z "${arch}" ]] && die "Architecture is required"
			EVENT_TYPE="new-ci-build-${arch}" trigger_dispatch
			;;
		build-asset)
			build_asset
			;;
		run-protected-tests)
			run_protected_tests
			;;
		*)
			die "Usage: $0 {check-workflow-trigger|trigger-protected-tests|trigger-arch-build <arch>|build-asset|run-protected-tests}"
			;;
	esac
}

main "$@"
