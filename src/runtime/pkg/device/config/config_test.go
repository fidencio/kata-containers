// Copyright (c) 2020 Intel Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

package config

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/opencontainers/runtime-spec/specs-go"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestInjectCDIDevices(t *testing.T) {
	for _, version := range []string{"0.6.0", "1.0.0", "1.1.0"} {
		t.Run(version, func(t *testing.T) {
			dir := t.TempDir()
			cdiSpec := fmt.Sprintf(`{
	"cdiVersion": %q,
	"kind": "nvidia.com/gpu",
	"containerEdits": {"env": ["CDI_COMMON=injected"]},
	"devices": [{
		"name": "test",
		"containerEdits": {
			"env": ["CDI_DEVICE=injected"],
			"deviceNodes": [{"path": "/dev/cdi-test", "type": "c", "major": 1, "minor": 3}]
		}
	}]
}`, version)
			require.NoError(t, os.WriteFile(filepath.Join(dir, "gpu.json"), []byte(cdiSpec), 0600))

			spec := &specs.Spec{Process: &specs.Process{Env: []string{"EXISTING=preserved"}}}
			require.NoError(t, injectDevices([]string{dir}, spec, []string{"nvidia.com/gpu=test"}))
			assert.Equal(t, []string{"EXISTING=preserved", "CDI_COMMON=injected", "CDI_DEVICE=injected"}, spec.Process.Env)
			require.NotNil(t, spec.Linux)
			require.Len(t, spec.Linux.Devices, 1)
			assert.Equal(t, "/dev/cdi-test", spec.Linux.Devices[0].Path)
			assert.Equal(t, "c", spec.Linux.Devices[0].Type)
			assert.Equal(t, int64(1), spec.Linux.Devices[0].Major)
			assert.Equal(t, int64(3), spec.Linux.Devices[0].Minor)
		})
	}
}

func TestWithCDINetDevices(t *testing.T) {
	dir := t.TempDir()
	cdiSpec := `{
	"cdiVersion": "1.1.0",
	"kind": "vendor.com/net",
	"devices": [{
		"name": "test",
		"containerEdits": {"netDevices": [{"hostInterfaceName": "eth0", "name": "net0"}]}
	}]
}`
	require.NoError(t, os.WriteFile(filepath.Join(dir, "net.json"), []byte(cdiSpec), 0600))
	spec := &specs.Spec{}
	result, err := WithCDI(map[string]string{"cdi.k8s.io/net": "vendor.com/net=test"}, []string{dir}, spec)
	require.NoError(t, err)
	require.Same(t, spec, result)
	require.NotNil(t, spec.Linux)
	assert.Equal(t, map[string]specs.LinuxNetDevice{"eth0": {Name: "net0"}}, spec.Linux.NetDevices)
}

func TestGetBackingFile(t *testing.T) {
	assert := assert.New(t)

	dir := t.TempDir()

	orgGetSysDevPath := getSysDevPath
	getSysDevPath = func(info DeviceInfo) string {
		return dir
	}
	defer func() { getSysDevPath = orgGetSysDevPath }()

	info := DeviceInfo{}
	path, err := getBackingFile(info)
	assert.Error(err)
	assert.Empty(path)

	loopDir := filepath.Join(dir, "loop")
	err = os.Mkdir(loopDir, os.FileMode(0755))
	assert.NoError(err)

	backingFile := "/fake-img"

	err = os.WriteFile(filepath.Join(loopDir, "backing_file"), []byte(backingFile), os.FileMode(0755))
	assert.NoError(err)

	path, err = getBackingFile(info)
	assert.NoError(err)
	assert.Equal(backingFile, path)
}

func TestGetSysDevPathImpl(t *testing.T) {
	assert := assert.New(t)

	info := DeviceInfo{
		DevType: "",
		Major:   127,
		Minor:   0,
	}

	path := getSysDevPathImpl(info)
	assert.Empty(path)

	expectedFormat := fmt.Sprintf("%d:%d", info.Major, info.Minor)

	info.DevType = "c"
	path = getSysDevPathImpl(info)
	assert.Contains(path, expectedFormat)
	assert.Contains(path, "char")

	info.DevType = "b"
	path = getSysDevPathImpl(info)
	assert.Contains(path, expectedFormat)
	assert.Contains(path, "block")
}

func TestIOMMUFDID(t *testing.T) {
	for _, tc := range []struct {
		devfsDev string
		expected string
	}{
		{"/dev/vfio/42", ""},
		{"/dev/vfio/devices/vfio99", "99"},
		{"/dev/vfio/invalid", ""},
		{"/dev/other/42", ""},
	} {
		t.Run(tc.devfsDev, func(t *testing.T) {
			assert := assert.New(t)

			info := VFIODev{
				DevfsDev: "/dev/vfio/devices/vfio5",
			}
			assert.Equal("5", info.IOMMUFDID())
		})
	}
}
