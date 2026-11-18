#!/usr/bin/env bats
# Copyright (c) 2026 NVIDIA Corporation
#
# SPDX-License-Identifier: Apache-2.0
#
# Helm template tests for k8sDistribution reaching the install. No cluster
# required.
#
# The value picks which host directory is mounted at /etc/containerd, while the
# install detects the node's CRI runtime itself and picks the file to write inside
# that mount. The install can only refuse a disagreement between the two if it is
# told what was declared, so these tests cover that it is.

# Bats provides BATS_TEST_DIRNAME when loading the suite.
# shellcheck disable=SC2154
load "${BATS_TEST_DIRNAME}/../../common.bash"

# shellcheck source=/dev/null
source "${BATS_TEST_DIRNAME}/lib/helm-deploy.bash"

CHART_PATH="$(get_chart_path)"

render() {
	helm template kata-deploy "${CHART_PATH}" \
		--set image.reference=quay.io/kata-containers/kata-deploy \
		--set image.tag=latest \
		"$@"
}

@test "Helm template: the declared distribution reaches the install, in both modes" {
	local mode rendered
	for mode in job daemonset; do
		rendered=$(render --set "deploymentMode=${mode}" --set k8sDistribution=k3s)

		echo "${rendered}" | grep -q 'name: K8S_DISTRIBUTION'
		echo "${rendered}" | grep -A1 'name: K8S_DISTRIBUTION' | grep -q 'value: "k3s"'
	done
}

@test "Helm template: pinning the containerd directory takes the check out of the way" {
	# containerd.configDir overrides the very derivation the install would be
	# checking, so an operator who set it is taken at their word. The flavour is
	# still declared: it decides the kubelet root directory too, which a pinned
	# containerd directory says nothing about.
	local rendered
	rendered=$(render --set deploymentMode=job \
		--set k8sDistribution=k0s \
		--set 'containerd.configDir=/etc/my-containerd/')

	echo "${rendered}" | grep -A1 'name: K8S_DISTRIBUTION' | grep -q 'value: "k0s"'
	echo "${rendered}" | grep -A1 'name: CONTAINERD_CONFIG_DIR' | grep -q 'value: "/etc/my-containerd/"'
}

@test "Helm template: the containerd directory is only reported when pinned" {
	local rendered
	rendered=$(render --set deploymentMode=job --set k8sDistribution=k0s)

	if echo "${rendered}" | grep -q 'name: CONTAINERD_CONFIG_DIR'; then
		echo "CONTAINERD_CONFIG_DIR was passed without an explicit containerd.configDir" >&2
		return 1
	fi
}

@test "Helm template: GKE uses the executable COS prefix for install and cleanup" {
	local mode rendered
	for mode in job daemonset; do
		rendered=$(render --set "deploymentMode=${mode}" --set k8sDistribution=gke \
			--set env.multiInstallSuffix=cos)

		echo "${rendered}" | grep -A1 'name: INSTALLATION_PREFIX' |
			grep -q 'value: "/home/kubernetes/bin"'
		echo "${rendered}" | grep -q 'mountPath: "/home/kubernetes/bin/opt/kata-cos"'
		echo "${rendered}" | grep -q 'path: "/home/kubernetes/bin/opt/kata-cos"'
		echo "${rendered}" | grep -A1 'name: K8S_DISTRIBUTION' | grep -q 'value: "gke"'
		if echo "${rendered}" | grep -q 'priorityClassName:'; then
			echo "GKE unexpectedly requested a priority class" >&2
			return 1
		fi
	done
}

@test "Helm template: GKE preserves an explicit installation prefix" {
	local rendered
	rendered=$(render --set k8sDistribution=gke --set env.installationPrefix=/custom)

	echo "${rendered}" | grep -A1 'name: INSTALLATION_PREFIX' | grep -q 'value: "/custom"'
	echo "${rendered}" | grep -q 'mountPath: "/custom/opt/kata"'
	echo "${rendered}" | grep -q 'path: "/custom/opt/kata"'
	if echo "${rendered}" | grep -q '/home/kubernetes/bin'; then
		echo "GKE default prefix overrode an explicit prefix" >&2
		return 1
	fi
}

@test "Helm template: other distributions keep their installation and priority defaults" {
	local mode rendered
	for mode in job daemonset; do
		rendered=$(render --set "deploymentMode=${mode}" --set monitor.enabled=true)

		echo "${rendered}" | grep -A1 'name: INSTALLATION_PREFIX' | grep -q 'value: ""'
		echo "${rendered}" | grep -q 'mountPath: "/opt/kata"'
		[[ "$(echo "${rendered}" | grep -c 'priorityClassName: "system-node-critical"')" -ge 2 ]]
	done
}

@test "Helm template: GKE priority overrides apply to every workload" {
	local rendered
	rendered=$(render --set k8sDistribution=gke --set monitor.enabled=true \
		--set job.reconcile.enabled=true --set priorityClassName=custom-priority)

	# Includes per-node install and cleanup Jobs, install/cleanup dispatchers,
	# scheduled reconciliation, and the optional monitor.
	[[ "$(echo "${rendered}" | grep -c 'priorityClassName: "custom-priority"')" -eq 6 ]]
	if echo "${rendered}" | grep -q 'system-node-critical'; then
		echo "distribution default overrode an explicit priority class" >&2
		return 1
	fi
}

@test "Helm template: an empty priority override disables priority on other distributions" {
	local rendered
	rendered=$(render --set priorityClassName= --set monitor.enabled=true \
		--set job.reconcile.enabled=true)

	if echo "${rendered}" | grep -q 'priorityClassName:'; then
		echo "empty priority override was ignored" >&2
		return 1
	fi
}

@test "Helm template: GKE never mounts absent COS binary directories, including cleanup" {
	local rendered
	rendered=$(render --set k8sDistribution=gke)

	echo "${rendered}" | grep -A2 'mountPath: /host-usr-local$' | grep -q 'readOnly: true'
	echo "${rendered}" | grep -A1 'path: /usr/local$' | grep -q 'type: Directory'
	if echo "${rendered}" | grep -Eq 'path: /usr/local/(sbin|bin)|name: node-binaries-remove'; then
		echo "GKE would try to create a directory on the read-only COS filesystem" >&2
		return 1
	fi
}

@test "Helm template: GKE rejects nodeBinaries before creating host jobs" {
	run render --set k8sDistribution=gke \
		--set nodeBinaries.extra.image=example/tools:latest \
		--set 'nodeBinaries.extra.binaries[0]=tool'

	[[ "${status}" -ne 0 ]]
	[[ "${output}" == *"nodeBinaries is not supported with k8sDistribution: gke"* ]]
}

@test "Helm template: GKE monitor and reconcile also omit priority by default" {
	local mode rendered
	for mode in job daemonset; do
		rendered=$(render --set "deploymentMode=${mode}" --set k8sDistribution=gke \
			--set monitor.enabled=true --set job.reconcile.enabled=true)

		if echo "${rendered}" | grep -q 'priorityClassName:'; then
			echo "GKE optional workloads unexpectedly requested a priority class" >&2
			return 1
		fi
	done
}

@test "Helm template: GKE DaemonSet and monitor preserve explicit priority" {
	local rendered
	rendered=$(render --set deploymentMode=daemonset --set k8sDistribution=gke \
		--set monitor.enabled=true --set priorityClassName=custom-priority)

	[[ "$(echo "${rendered}" | grep -c 'priorityClassName: "custom-priority"')" -eq 2 ]]
}
