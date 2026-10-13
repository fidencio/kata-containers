// Copyright 2025 Kata Contributors
//
// SPDX-License-Identifier: Apache-2.0
//

use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io;
use std::io::prelude::*;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use nix::unistd::gettid;

/// Check if SELinux is enabled on the system
pub fn is_selinux_enabled() -> bool {
    fs::read_to_string("/proc/mounts")
        .map(|buf| buf.contains("selinuxfs"))
        .unwrap_or_default()
}

pub fn set_exec_label(label: &str) -> Result<()> {
    let mut attr_path = Path::new("/proc/thread-self/attr/exec").to_path_buf();
    if !attr_path.exists() {
        // Fall back to the old convention
        attr_path = Path::new("/proc/self/task")
            .join(gettid().to_string())
            .join("attr/exec")
    }

    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(attr_path)
        .context("open attr path")?;

    file.write_all(label.as_bytes())
        .with_context(|| "failed to apply SELinux label")?;

    Ok(())
}

/// Set the SELinux label the child will use when it executes the command.
/// Apply it after fork so the shim thread does not pass the VM label to other
/// programs, such as userdel. If setting the label fails, abort the launch.
pub fn set_exec_label_for_command(command: &mut Command, label: &str) -> io::Result<()> {
    // Allocate the label before fork. In the child, allocating or logging
    // could deadlock on a lock held by another thread in the parent.
    let label = CString::new(label)?;
    unsafe {
        command.pre_exec(move || {
            let mut fd = libc::open(
                b"/proc/thread-self/attr/exec\0".as_ptr().cast(),
                libc::O_WRONLY | libc::O_CLOEXEC,
            );
            if fd < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
                // Older kernels may not provide /proc/thread-self. The child
                // has only one thread, so /proc/self/attr/exec sets its label too.
                fd = libc::open(
                    b"/proc/self/attr/exec\0".as_ptr().cast(),
                    libc::O_WRONLY | libc::O_CLOEXEC,
                );
            }
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }

            let bytes = label.as_bytes();
            let result = loop {
                let written = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
                if written < 0 {
                    let err = io::Error::last_os_error();
                    if err.raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    break Err(err);
                }
                if written as usize != bytes.len() {
                    break Err(io::Error::from_raw_os_error(libc::EIO));
                }
                break Ok(());
            };
            libc::close(fd);
            result
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_rejects_label_with_nul() {
        let mut command = Command::new("true");
        let err = set_exec_label_for_command(&mut command, "invalid\0label").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn command_label_does_not_leak_to_parent_threads() {
        if !is_selinux_enabled() {
            return;
        }
        // Reuse the test process's label instead of assuming the host policy
        // provides a particular label. Configure the command here, then launch
        // it on other threads to check that the label follows the command when
        // Tokio moves work between threads.
        let label = fs::read_to_string("/proc/self/attr/current").unwrap();
        let label = label.trim_end_matches(['\0', '\n']);
        let mut command = Command::new("cat");
        command.arg("/proc/self/attr/current");
        set_exec_label_for_command(&mut command, label).unwrap();

        for _ in 0..2 {
            command = std::thread::spawn(move || {
                let pending = fs::read("/proc/thread-self/attr/exec").unwrap();
                let output = command.output().unwrap();
                assert!(output.status.success());
                assert_eq!(fs::read("/proc/thread-self/attr/exec").unwrap(), pending);
                let current = String::from_utf8(output.stdout).unwrap();
                let parent = fs::read_to_string("/proc/thread-self/attr/current").unwrap();
                assert_eq!(
                    current.trim_end_matches(['\0', '\n']),
                    parent.trim_end_matches(['\0', '\n'])
                );
                command
            })
            .join()
            .unwrap();
        }
    }

    #[test]
    fn command_aborts_on_label_failure() {
        if !is_selinux_enabled() {
            return;
        }
        let pending = fs::read("/proc/thread-self/attr/exec").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("executed");
        let mut command = Command::new("touch");
        command.arg(&marker);
        set_exec_label_for_command(&mut command, "invalid-selinux-context").unwrap();
        assert!(command.spawn().is_err());
        assert!(!marker.exists());
        assert_eq!(fs::read("/proc/thread-self/attr/exec").unwrap(), pending);
    }
}
