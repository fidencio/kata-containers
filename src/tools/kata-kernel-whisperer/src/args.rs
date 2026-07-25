// Copyright (c) Kata Containers Contributors
//
// SPDX-License-Identifier: Apache-2.0

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "kata-kernel-whisperer",
    about = "Print the config-derived QEMU guest kernel command line for the Kata Rust runtime",
    long_about = "Print the guest kernel command line assembled from a runtime-rs configuration \
                  file and its config.d drop-ins (for example debug overlays installed by \
                  kata-deploy).\n\n\
                  Pass individual --config paths, or --kata-root to discover every installed \
                  runtime-rs and custom-runtime configuration under that installation prefix.\n\n\
                  This is the static, config-derived cmdline only. Pod-specific cold-plugged \
                  devices may later contribute additional guest parameters that are not \
                  reflected here."
)]
pub(crate) struct Args {
    /// Runtime configuration file. May be specified more than once.
    #[arg(short, long)]
    pub(crate) config: Vec<PathBuf>,

    /// Kata installation root (for example /opt/kata). Discovers configuration
    /// files under share/defaults/kata-containers/runtime-rs/runtimes and
    /// share/defaults/kata-containers/custom-runtimes.
    #[arg(long, value_name = "DIR")]
    pub(crate) kata_root: Option<PathBuf>,

    /// Emit a JSON array with runtime class, architecture, config path, and cmdline.
    #[arg(long)]
    pub(crate) json: bool,
}
