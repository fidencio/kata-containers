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
                  file and its config.d drop-ins (for example the debug and devkit overlays \
                  installed by kata-deploy).\n\n\
                  Pass individual --config paths, or --kata-root to describe every runtime-rs \
                  RuntimeClass of an installation. RuntimeClasses served by the Go runtime are \
                  out of scope and left out; runtime-rs ones on a hypervisor other than QEMU are \
                  reported as skipped rather than omitted, so a consumer can tell them from an \
                  entry that has gone missing.\n\n\
                  This is the static, config-derived cmdline only. Pod-specific cold-plugged \
                  devices may later contribute additional guest parameters that are not \
                  reflected here."
)]
pub(crate) struct Args {
    /// Runtime configuration file. May be specified more than once.
    #[arg(short, long)]
    pub(crate) config: Vec<PathBuf>,

    /// Kata installation root (for example /opt/kata). Reads the RuntimeClass
    /// manifest written by `kata-deploy render-configs`, falling back to
    /// discovery by directory layout on installations that have none.
    #[arg(long, value_name = "DIR")]
    pub(crate) kata_root: Option<PathBuf>,

    /// Emit the report as JSON: architecture, one entry per described
    /// RuntimeClass, and the ones that were skipped.
    #[arg(long)]
    pub(crate) json: bool,
}
