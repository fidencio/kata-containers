// Copyright (c) 2017 Intel Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

package docker

const (
	// Annotations set by Docker (the daemon) or historically by the Kubernetes
	// dockershim. Used to identify container type (sandbox vs container),
	// sandbox ID, and network namespace.

	// ContainerTypeLabelKey is the container type (podsandbox or container) annotation
	ContainerTypeLabelKey = "io.kubernetes.docker.type"

	// ContainerTypeLabelSandbox represents a sandbox sandbox container
	ContainerTypeLabelSandbox = "podsandbox"

	// ContainerTypeLabelContainer represents a container running within a sandbox
	ContainerTypeLabelContainer = "container"

	// SandboxIDLabelKey is the sandbox ID annotation
	SandboxIDLabelKey = "io.kubernetes.sandbox.id"

	// NetworkNamespacePathKey is set by Docker when using a runtime that needs
	// the sandbox netns (e.g. Kata). The value is the libnetwork sandbox key
	// (netns path). Kata uses it as NetworkConfig.NetworkID so the VM uses
	// that netns (where the veth was placed) instead of creating a new one.
	// Must match Docker's DockerNetworkNamespacePathAnnotation (com.docker.network.namespace.path).
	NetworkNamespacePathKey = "com.docker.network.namespace.path"
)
