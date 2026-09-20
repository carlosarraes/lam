use anyhow::{ensure, Result};
use std::{ffi::CStr, os::unix::ffi::OsStringExt, path::PathBuf};

#[repr(C)]
struct RawSnapshot {
    start_usec: u64,
    pid: u32,
    parent: u32,
    uid: u32,
    executable: [u8; 4096],
    boot_id: [u8; 40],
}

unsafe extern "C" {
    fn lam_chat_process_snapshot(pid: libc::c_int, out: *mut RawSnapshot) -> libc::c_int;
    fn lam_chat_process_arguments(
        pid: libc::c_int,
        bytes: *mut u8,
        length: *mut libc::size_t,
    ) -> libc::c_int;
    fn lam_chat_process_owner(pid: libc::c_int, uid: *mut u32) -> libc::c_int;
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessEvidence {
    pub pid: u32,
    pub parent: u32,
    pub uid: u32,
    pub boot_id: String,
    pub start_usec: u64,
    pub executable: PathBuf,
}

impl PartialEq for ProcessEvidence {
    fn eq(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.uid == other.uid
            && self.boot_id == other.boot_id
            && self.start_usec == other.start_usec
            && self.executable == other.executable
    }
}

impl Eq for ProcessEvidence {}

impl ProcessEvidence {
    pub fn owner(pid: u32) -> Result<u32> {
        ensure!(pid > 0 && pid <= i32::MAX as u32, "invalid process ID");
        let mut uid = 0;
        if unsafe { lam_chat_process_owner(pid as libc::c_int, &mut uid) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(uid)
    }

    pub fn start_marker(&self) -> u64 {
        self.start_usec
    }
    pub fn args(pid: u32) -> Result<Vec<Vec<u8>>> {
        ensure!(pid > 0 && pid <= i32::MAX as u32, "invalid process ID");
        let mut bytes = vec![0_u8; 1_048_576];
        let mut length = bytes.len();
        let result = unsafe {
            lam_chat_process_arguments(pid as libc::c_int, bytes.as_mut_ptr(), &mut length)
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        ensure!(
            length >= 6 && length <= bytes.len(),
            "invalid process argument length"
        );
        bytes.truncate(length);
        let argc = i32::from_ne_bytes(bytes[..4].try_into()?);
        ensure!((1..=128).contains(&argc), "invalid native argument count");
        let mut cursor = 4;
        let exe_end = bytes[cursor..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| anyhow::anyhow!("missing native executable argument"))?;
        cursor += exe_end + 1;
        while cursor < bytes.len() && bytes[cursor] == 0 {
            cursor += 1;
        }
        let mut args = Vec::with_capacity(argc as usize);
        let mut total = 0;
        for _ in 0..argc {
            ensure!(cursor < bytes.len(), "truncated native arguments");
            let end = bytes[cursor..]
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| anyhow::anyhow!("unterminated native argument"))?;
            total += end + 1;
            ensure!(total <= 65_536, "native arguments exceed validation bound");
            args.push(bytes[cursor..cursor + end].to_vec());
            cursor += end + 1;
        }
        ensure!(!args[0].is_empty(), "missing native program argument");
        Ok(args)
    }

    pub fn read(pid: u32) -> Result<Self> {
        ensure!(pid > 0 && pid <= i32::MAX as u32, "invalid process ID");
        let mut raw: RawSnapshot = unsafe { std::mem::zeroed() };
        let result = unsafe { lam_chat_process_snapshot(pid as libc::c_int, &mut raw) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let executable = PathBuf::from(std::ffi::OsString::from_vec(
            CStr::from_bytes_until_nul(&raw.executable)?
                .to_bytes()
                .to_vec(),
        ));
        let boot_id =
            uuid::Uuid::parse_str(CStr::from_bytes_until_nul(&raw.boot_id)?.to_str()?)?.to_string();
        ensure!(
            raw.pid == pid && raw.parent > 0 && raw.parent != pid && raw.start_usec > 0,
            "invalid process lifetime evidence"
        );
        ensure!(
            raw.uid == unsafe { libc::geteuid() },
            "process belongs to another user"
        );
        ensure!(
            executable.is_absolute(),
            "process executable is not absolute"
        );
        crate::chat::protocol::validate_uuid(&boot_id)?;
        Ok(Self {
            pid,
            parent: raw.parent,
            uid: raw.uid,
            boot_id,
            start_usec: raw.start_usec,
            executable,
        })
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(Self::read(self.pid)? == *self, "process lifetime changed");
        Ok(())
    }

    pub fn validate_descendant(&self, mut pid: u32) -> Result<()> {
        self.validate()?;
        for _ in 0..64 {
            if pid == self.pid {
                return self.validate();
            }
            ensure!(pid > 1, "command is outside its native process");
            let observed = Self::read(pid)?;
            ensure!(observed.parent != pid, "invalid process ancestry");
            pid = observed.parent;
        }
        anyhow::bail!("process ancestry exceeds validation bound")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn current_process_has_stable_kernel_lifetime_and_parent() {
        let first = ProcessEvidence::read(std::process::id()).unwrap();
        assert_eq!(first.pid, std::process::id());
        assert_eq!(first.parent, unsafe { libc::getppid() } as u32);
        assert_eq!(first.uid, unsafe { libc::geteuid() });
        assert!(uuid::Uuid::parse_str(&first.boot_id).is_ok());
        assert!(first.start_usec > 0);
        assert_eq!(
            first.executable.canonicalize().unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
        assert_eq!(ProcessEvidence::read(std::process::id()).unwrap(), first);
        let mut reparented = first.clone();
        reparented.parent = 1;
        assert_eq!(first, reparented);
        first.validate().unwrap();
        first.validate_descendant(std::process::id()).unwrap();
        assert!(first.validate_descendant(1).is_err());
    }

    #[test]
    fn child_process_has_distinct_lifetime_and_is_a_descendant() {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let child = ChildGuard(
            std::process::Command::new("/bin/sleep")
                .arg("5")
                .spawn()
                .unwrap(),
        );
        let parent = ProcessEvidence::read(std::process::id()).unwrap();
        let child_process = ProcessEvidence::read(child.0.id()).unwrap();
        assert_eq!(child_process.parent, parent.pid);
        assert_ne!(child_process.start_usec, parent.start_usec);
        parent.validate_descendant(child.0.id()).unwrap();
        assert!(child_process.validate_descendant(parent.pid).is_err());
    }

    #[test]
    fn missing_process_is_rejected() {
        assert!(ProcessEvidence::read(0).is_err());
        assert!(ProcessEvidence::read(i32::MAX as u32).is_err());
    }

    #[test]
    fn native_argv_is_read_from_the_exact_kernel_process() {
        let args = ProcessEvidence::args(std::process::id()).unwrap();
        assert!(!args.is_empty());
        assert!(std::path::Path::new(std::ffi::OsStr::from_bytes(&args[0])).is_absolute());
        assert!(ProcessEvidence::args(0).is_err());
    }

    #[test]
    fn native_argv_can_be_read_from_same_user_parent() {
        let parent = unsafe { libc::getppid() } as u32;
        let process = ProcessEvidence::read(parent).unwrap();
        assert_eq!(process.uid, unsafe { libc::geteuid() });
        let args = ProcessEvidence::args(parent).unwrap();
        assert!(!args.is_empty());
    }
}
