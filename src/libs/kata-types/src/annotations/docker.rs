// Copyright (c) 2019 Alibaba Cloud
// Copyright (c) 2019 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

#![allow(missing_docs)]

//! Annotations set by Docker (the daemon) or historically by the Kubernetes dockershim.
//! Used to identify container type (sandbox vs container), sandbox ID, and network namespace.

///  ContainerTypeLabelKey is the container type (podsandbox or container) of key.
pub const CONTAINER_TYPE_LABEL_KEY: &str = "io.kubernetes.docker.type";

/// ContainerTypeLabelSandbox represents a sandbox sandbox container.
pub const SANDBOX: &str = "podsandbox";

/// ContainerTypeLabelContainer represents a container running within a sandbox.
pub const CONTAINER: &str = "container";

/// SandboxIDLabelKey is the sandbox ID annotation.
pub const SANDBOX_ID_LABEL_KEY: &str = "io.kubernetes.sandbox.id";

/// NetworkNamespacePathKey is set by Docker when using a runtime that needs the sandbox netns
/// (e.g. Kata). The value is the libnetwork sandbox key (netns path). Kata uses it so the VM
/// uses that netns (where the veth was placed) instead of creating a new one.
/// Must match Docker's DockerNetworkNamespacePathAnnotation (com.docker.network.namespace.path).
pub const NETWORK_NAMESPACE_PATH_KEY: &str = "com.docker.network.namespace.path";
