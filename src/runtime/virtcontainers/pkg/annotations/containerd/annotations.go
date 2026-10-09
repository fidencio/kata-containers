// Copyright (c) 2026 NVIDIA Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

package containerd

// CRI annotations consumed by Kata. Keep these values in sync with containerd's
// internal/cri/annotations package.
const (
	ContainerTypeSandbox   = "sandbox"
	ContainerTypeContainer = "container"
	ContainerType          = "io.kubernetes.cri.container-type"
	SandboxID              = "io.kubernetes.cri.sandbox-id"
	SandboxName            = "io.kubernetes.cri.sandbox-name"
	SandboxNamespace       = "io.kubernetes.cri.sandbox-namespace"
	SandboxCPUPeriod       = "io.kubernetes.cri.sandbox-cpu-period"
	SandboxCPUQuota        = "io.kubernetes.cri.sandbox-cpu-quota"
	SandboxCPUShares       = "io.kubernetes.cri.sandbox-cpu-shares"
	SandboxMem             = "io.kubernetes.cri.sandbox-memory"
)
