#!/usr/bin/env bash
#
# Copyright (c) 2025 NVIDIA Corporation
#
# SPDX-License-Identifier: Apache-2.0
#
# Shared functions for nydus-snapshotter stability tests.
# Used by the GitHub Actions workflows for testing nydus-snapshotter
# with Kata Containers in different scenarios.

set -o errexit
set -o nounset
set -o pipefail

DEBUG="${DEBUG:-}"
[[ -n "${DEBUG}" ]] && set -x

kubernetes_dir="$(dirname "$(readlink -f "$0")")"

# Default configuration - must be exported for gha-run.sh to see them
export KATA_HYPERVISOR="${KATA_HYPERVISOR:-qemu-coco-dev}"
export KUBERNETES="${KUBERNETES:-vanilla}"
export SNAPSHOTTER="${SNAPSHOTTER:-nydus}"
export USE_EXPERIMENTAL_SETUP_SNAPSHOTTER="${USE_EXPERIMENTAL_SETUP_SNAPSHOTTER:-true}"
# guest-pull enables FS_DRIVER=proxy in nydus-snapshotter for Kata guest pulling
export PULL_TYPE="${PULL_TYPE:-guest-pull}"
# DOCKER_TAG should be set by the workflow to select patched/non-patched kata-deploy image
export DOCKER_TAG="${DOCKER_TAG:-kata-containers-latest}"
POD_TIMEOUT="${POD_TIMEOUT:-10m}"

# Default images to test
# Note: Images must have basic shell utilities (uname). Distroless/scratch images won't work.
# Using SHA256 digests for reproducibility.
IMAGES_LIST="${IMAGES_LIST:-quay.io/mongodb/mongodb-community-server@sha256:8b73733842da21b6bbb6df4d7b2449229bb3135d2ec8c6880314d88205772a11 ghcr.io/edgelesssys/redis@sha256:ecb0a964c259a166a1eb62f0eb19621d42bd1cce0bc9bb0c71c828911d4ba93d}"

# Run a pod with kubectl and wait for completion
# Arguments:
#   $1 - pod name
#   $2 - image
#   $3 - runtime class (empty for default/runc)
#   $4 - command to run (optional, defaults to "uname -r")
run_pod() {
    local pod_name="$1"
    local image="$2"
    local runtime_class="${3:-}"
    local cmd="${4:-uname -r}"

    echo "Running pod: ${pod_name} with image: ${image}"

    # Clean up any leftover pod from previous runs
    kubectl delete pod "${pod_name}" --ignore-not-found=true 2>/dev/null || true

    local overrides=""
    if [[ -n "${runtime_class}" ]]; then
        overrides="--overrides={\"spec\":{\"runtimeClassName\":\"${runtime_class}\"}}"
    fi

    # shellcheck disable=SC2086
    # Note: Using -i (not -it) because -t requires a TTY which isn't available in CI
    kubectl run "${pod_name}" \
        -i --rm \
        --restart=Never \
        --image="${image}" \
        --image-pull-policy=Always \
        --pod-running-timeout="${POD_TIMEOUT}" \
        ${overrides} \
        -- ${cmd}
}

# Run pods with Kata runtime
# Uses nydus snapshotter for image pulling
run_kata_pods() {
    local prefix="${1:-kata}"
    local runtime_class="${2:-kata-${KATA_HYPERVISOR}}"

    echo "=== Running Kata pods with runtime: ${runtime_class} ==="
    # shellcheck disable=SC2086
    for img in ${IMAGES_LIST}; do
        # Pod name must be <= 63 chars. Reserve space for prefix + "-"
        local max_img_len=$((62 - ${#prefix}))
        local img_suffix
        img_suffix=$(echo "${img}" | tr ':.@/' '-' | cut -c1-${max_img_len})
        local pod_name="${prefix}-${img_suffix}"
        run_pod "${pod_name}" "${img}" "${runtime_class}"
    done
}

# Run pods with runc (default runtime)
# Uses overlayfs snapshotter
run_runc_pods() {
    local prefix="${1:-runc}"

    echo "=== Running runc pods (overlayfs) ==="
    # shellcheck disable=SC2086
    for img in ${IMAGES_LIST}; do
        # Pod name must be <= 63 chars. Reserve space for prefix + "-"
        local max_img_len=$((62 - ${#prefix}))
        local img_suffix
        img_suffix=$(echo "${img}" | tr ':.@/' '-' | cut -c1-${max_img_len})
        local pod_name="${prefix}-${img_suffix}"
        run_pod "${pod_name}" "${img}" ""
    done
}

# Adjust Kata Containers configuration
# Increases timeouts and memory for guest pulling
adjust_kata_config() {
    local config_file="/opt/kata/share/defaults/kata-containers/configuration-${KATA_HYPERVISOR}.toml"

    echo "=== Adjusting Kata configuration ==="

    # Increase create_container_timeout for guest pulling
    sudo sed -i -e 's/^\(create_container_timeout\).*=.*$/\1 = 600/g' "${config_file}"
    grep "create_container_timeout.*=" "${config_file}"

    # Increase default_memory for tmpfs space during guest pulling
    sudo sed -i -e 's/^\(default_memory\).*=.*$/\1 = 4096/g' "${config_file}"
    grep "default_memory.*=" "${config_file}"
}

# Check logs for known nydus-snapshotter issues
# Returns non-zero if critical issues are found
check_for_issues() {
    local fail_on_critical="${1:-true}"

    echo "=============================================="
    echo "Checking for known nydus-snapshotter issues"
    echo "=============================================="

    local nydus_log
    local containerd_log
    nydus_log=$(sudo journalctl -u nydus-snapshotter --no-pager 2>/dev/null || true)
    containerd_log=$(sudo journalctl -u containerd --no-pager 2>/dev/null || true)

    local issue_found=0

    # Issue #680: Missing parent bucket
    echo ""
    echo "=== Checking for 'missing parent bucket' (Issue #680) ==="
    local missing_parent
    missing_parent=$(echo "${nydus_log}" "${containerd_log}" | grep -c "missing parent.*bucket.*not found" || true)
    if [[ "${missing_parent}" -gt 0 ]]; then
        echo "FOUND: 'missing parent bucket: not found' errors: ${missing_parent}"
        echo "${nydus_log}" "${containerd_log}" | grep "missing parent.*bucket.*not found" | tail -20
        issue_found=1
    else
        echo "No 'missing parent bucket' errors found"
    fi

    # Check for lazy recovery success (patched version)
    echo ""
    echo "=== Checking for lazy parent recovery (patched fix) ==="
    local recovery
    recovery=$(echo "${nydus_log}" | grep -c "lazy recovery\|recovered parent\|nydus.recovered" || true)
    if [[ "${recovery}" -gt 0 ]]; then
        echo "FOUND: Lazy parent recovery was triggered: ${recovery} times"
        echo "${nydus_log}" | grep "lazy recovery\|recovered parent\|nydus.recovered" | tail -10
    else
        echo "No lazy recovery messages found (may not have been needed)"
    fi

    # Issue: Instance already exists
    echo ""
    echo "=== Checking for 'already exists' errors ==="
    local already_exists
    already_exists=$(echo "${nydus_log}" | grep -c "already exists\|has associated" || true)
    if [[ "${already_exists}" -gt 0 ]]; then
        echo "FOUND: 'already exists' warnings/errors: ${already_exists}"
        echo "${nydus_log}" | grep "already exists\|has associated" | tail -10
    else
        echo "No 'already exists' errors found"
    fi

    # General snapshotter errors
    echo ""
    echo "=== Checking for general snapshotter errors ==="
    local snap_errors
    snap_errors=$(echo "${nydus_log}" "${containerd_log}" | grep -ci "error.*snapshot\|snapshot.*error\|failed.*snapshot\|failed to create.*snapshot" || true)
    if [[ "${snap_errors}" -gt 0 ]]; then
        echo "FOUND: Snapshotter-related errors: ${snap_errors}"
        echo "${nydus_log}" "${containerd_log}" | grep -i "error.*snapshot\|snapshot.*error\|failed.*snapshot\|failed to create.*snapshot" | tail -15
    else
        echo "No general snapshotter errors found"
    fi

    # Content digest not found
    echo ""
    echo "=== Checking for 'content digest not found' errors ==="
    local digest_errors
    digest_errors=$(echo "${containerd_log}" | grep -c "content digest.*not found" || true)
    if [[ "${digest_errors}" -gt 0 ]]; then
        echo "FOUND: 'content digest not found' errors: ${digest_errors}"
        echo "${containerd_log}" | grep "content digest.*not found" | tail -10
        issue_found=1
    else
        echo "No 'content digest not found' errors found"
    fi

    echo ""
    echo "=============================================="
    echo "Summary of detected issues"
    echo "=============================================="
    echo "Missing parent bucket errors: ${missing_parent}"
    echo "Already exists warnings: ${already_exists}"
    echo "General snapshotter errors: ${snap_errors}"
    echo "Content digest errors: ${digest_errors}"
    echo "Lazy recovery triggered: ${recovery}"

    if [[ "${fail_on_critical}" == "true" && "${issue_found}" -eq 1 ]]; then
        echo ""
        echo "CRITICAL ISSUES DETECTED"
        return 1
    fi

    return 0
}

# Collect full logs (for debugging on failure)
collect_logs() {
    echo "=== Full nydus-snapshotter logs ==="
    sudo journalctl -u nydus-snapshotter --no-pager -n 500 || true
    echo ""
    echo "=== Full containerd logs ==="
    sudo journalctl -u containerd --no-pager -n 300 || true
}

# Deploy Kata Containers using gha-run.sh
deploy_kata() {
    echo "=== Deploying Kata Containers ==="
    bash "${kubernetes_dir}/gha-run.sh" deploy-kata
}

# Cleanup/uninstall Kata Containers
cleanup_kata() {
    echo "=== Cleaning up Kata Containers ==="
    bash "${kubernetes_dir}/gha-run.sh" cleanup || true
}

# Main entry point
# Usage: nydus_tests.sh <action> [args...]
# Actions:
#   run-kata-pods [prefix] [runtime_class]
#   run-runc-pods [prefix]
#   adjust-config
#   check-issues [fail_on_critical]
#   collect-logs
#   deploy-kata
#   cleanup-kata
main() {
    local action="${1:-help}"
    shift || true

    case "${action}" in
        run-kata-pods)
            run_kata_pods "$@"
            ;;
        run-runc-pods)
            run_runc_pods "$@"
            ;;
        adjust-config)
            adjust_kata_config
            ;;
        check-issues)
            check_for_issues "$@"
            ;;
        collect-logs)
            collect_logs
            ;;
        deploy-kata)
            deploy_kata
            ;;
        cleanup-kata)
            cleanup_kata
            ;;
        help|*)
            echo "Usage: $0 <action> [args...]"
            echo ""
            echo "Actions:"
            echo "  run-kata-pods [prefix] [runtime_class]  - Run pods with Kata runtime"
            echo "  run-runc-pods [prefix]                  - Run pods with runc (overlayfs)"
            echo "  adjust-config                           - Adjust Kata configuration"
            echo "  check-issues [fail_on_critical]         - Check logs for known issues"
            echo "  collect-logs                            - Collect full debug logs"
            echo "  deploy-kata                             - Deploy Kata Containers"
            echo "  cleanup-kata                            - Cleanup Kata Containers"
            echo ""
            echo "Environment variables:"
            echo "  KATA_HYPERVISOR                    - Kata hypervisor (default: qemu-coco-dev)"
            echo "  KUBERNETES                         - K8s distribution (default: vanilla)"
            echo "  SNAPSHOTTER                        - Snapshotter to use (default: nydus)"
            echo "  USE_EXPERIMENTAL_SETUP_SNAPSHOTTER - Enable experimental snapshotter setup (default: true)"
            echo "  PULL_TYPE                          - Pull type: default or guest-pull (default: guest-pull)"
            echo "  DOCKER_TAG                         - kata-deploy image tag (default: kata-containers-latest)"
            echo "  IMAGES_LIST                        - Space-separated list of images to test"
            echo "  POD_TIMEOUT                        - Pod running timeout (default: 10m)"
            ;;
    esac
}

# Only run main if script is executed directly (not sourced)
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi

