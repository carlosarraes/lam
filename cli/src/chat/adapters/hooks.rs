use std::time::Instant;

pub(super) fn read_input(deadline: Instant) -> anyhow::Result<Vec<u8>> {
    let mut input = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| anyhow::anyhow!("native hook input deadline expired"))?;
        let mut poll = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe {
            libc::poll(
                &mut poll,
                1,
                remaining.as_millis().min(i32::MAX as u128) as i32,
            )
        };
        if ready < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        anyhow::ensure!(ready > 0, "native hook input deadline expired");
        let mut bytes = [0u8; 4096];
        let count =
            unsafe { libc::read(libc::STDIN_FILENO, bytes.as_mut_ptr().cast(), bytes.len()) };
        if count < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        anyhow::ensure!(count >= 0, "native hook input unavailable");
        if count == 0 {
            return Ok(input);
        }
        input.extend_from_slice(&bytes[..count as usize]);
        anyhow::ensure!(
            input.len() <= 1_048_576,
            "native hook input exceeds its bound"
        );
    }
}

pub(super) fn record_error(client: &str, stage: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::{
        io::Write,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let paths = crate::chat::config::Paths::discover()?;
    let directory = paths
        .database
        .parent()
        .context("missing Chat data directory")?;
    crate::chat::config::validate_path_components(directory)?;
    crate::chat::config::ensure_private_dir(directory, "Chat hook diagnostics")?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(directory.join("hook-errors.jsonl"))?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o777 == 0o600
            && metadata.nlink() == 1
            && metadata.len() < 65_536,
        "private hook diagnostic unavailable"
    );
    anyhow::ensure!(matches!(client, "codex" | "claude"), "invalid hook client");
    anyhow::ensure!(
        matches!(stage, "input" | "binding" | "output"),
        "invalid hook diagnostic stage"
    );
    let mut line =
        serde_json::to_vec(&serde_json::json!({"client":client,"stage":stage,"status":"error"}))?;
    line.push(b'\n');
    file.write_all(&line)?;
    Ok(())
}
