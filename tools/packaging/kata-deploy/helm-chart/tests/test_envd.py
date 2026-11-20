#!/usr/bin/env python3
# Copyright (c) 2026 NVIDIA Corporation
# SPDX-License-Identifier: Apache-2.0

"""Render-only chart tests; requires helm and PyYAML, never contacts a cluster."""

import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import yaml


CHART = Path(__file__).resolve().parents[1] / "kata-deploy"


def render(envd=False, shims=None, **settings):
    values = {
        "envd": envd,
        "shims": {"disableAll": True, **(shims or {"qemu-runtime-rs": {"enabled": True}})},
        "runtimeClasses": {"createDefault": False},
        **settings,
    }
    with tempfile.NamedTemporaryFile(mode="w", suffix=".json") as file:
        json.dump(values, file)
        file.flush()
        output = subprocess.check_output(
            ["helm", "template", "test", str(CHART), "-f", file.name], text=True
        )
    docs = list(yaml.safe_load_all(output))
    classes = {
        doc["metadata"]["name"]: doc
        for doc in docs
        if doc and doc.get("kind") == "RuntimeClass"
    }
    return output, classes


class EnvdChartTests(unittest.TestCase):
    def test_disabled_by_default(self):
        output, classes = render()
        self.assertNotIn("name: ENVD", output)
        self.assertEqual(set(classes), {"kata-qemu-runtime-rs"})

    def test_enabled_preserves_plain_runtimeclass(self):
        _, plain = render()
        output, enabled = render(envd=True)
        self.assertIn("name: ENVD", output)
        self.assertEqual(plain["kata-qemu-runtime-rs"], enabled["kata-qemu-runtime-rs"])
        variant = enabled["kata-qemu-runtime-rs-envd"]
        self.assertEqual(variant["handler"], "kata-qemu-runtime-rs-envd")
        self.assertEqual(variant["overhead"], plain["kata-qemu-runtime-rs"]["overhead"])
        self.assertEqual(variant["scheduling"], plain["kata-qemu-runtime-rs"]["scheduling"])

    def test_only_supported_rust_hypervisors(self):
        shims = {name: {"enabled": True} for name in [
            "qemu", "qemu-runtime-rs", "clh", "clh-runtime-rs", "dragonball"
        ]}
        _, classes = render(envd=True, shims=shims)
        self.assertEqual(
            {name for name in classes if name.endswith("-envd")},
            {"kata-qemu-runtime-rs-envd", "kata-clh-runtime-rs-envd", "kata-dragonball-envd"},
        )

    def test_unsupported_arch_has_no_envd_runtimeclass(self):
        _, classes = render(envd=True, shims={
            "qemu-runtime-rs": {"enabled": True, "supportedArches": ["s390x"]}
        })
        self.assertFalse(any(name.endswith("-envd") for name in classes))

    def test_both_deployment_modes_receive_the_flag(self):
        for mode in ["job", "daemonset"]:
            with self.subTest(mode=mode):
                output, classes = render(envd=True, deploymentMode=mode)
                self.assertIn("name: ENVD", output)
                self.assertIn("kata-qemu-runtime-rs-envd", classes)

    def test_variants_coexist_and_honor_multi_install_suffix(self):
        _, classes = render(
            envd=True, debug=True, devkit=True, env={"multiInstallSuffix": "blue"}
        )
        prefix = "kata-qemu-runtime-rs-blue"
        self.assertEqual(set(classes), {prefix, f"{prefix}-debug", f"{prefix}-devkit", f"{prefix}-envd"})
        self.assertTrue(all(name == entry["handler"] for name, entry in classes.items()))


if __name__ == "__main__":
    unittest.main()
