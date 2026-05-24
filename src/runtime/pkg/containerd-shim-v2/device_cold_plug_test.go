// Copyright (c) 2025 NVIDIA CORPORATION.
//
// SPDX-License-Identifier: Apache-2.0
//

package containerdshim

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/assert"
)

func TestKubeletPodResourceSocketAvailable(t *testing.T) {
	assert.False(t, kubeletPodResourceSocketAvailable(""))

	tmpDir := t.TempDir()
	sockPath := filepath.Join(tmpDir, "kubelet.sock")
	assert.False(t, kubeletPodResourceSocketAvailable(sockPath))

	f, err := os.Create(sockPath)
	assert.NoError(t, err)
	assert.NoError(t, f.Close())

	assert.True(t, kubeletPodResourceSocketAvailable(sockPath))
}
