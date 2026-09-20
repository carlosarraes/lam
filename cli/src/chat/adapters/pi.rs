#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HookInput {
    pub session_id: String,
    pub cwd: std::path::PathBuf,
    pub hook_event_name: String,
}

impl HookInput {
    fn parse(bytes: &[u8], event: &str, native_id: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            bytes.len() <= 1_048_576,
            "native extension input exceeds bound"
        );
        let input: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("invalid native extension input"))?;
        anyhow::ensure!(
            input.hook_event_name == event && matches!(event, "SessionStart" | "SessionEnd"),
            "native extension event mismatch"
        );
        crate::chat::protocol::validate_uuid(native_id)?;
        anyhow::ensure!(input.session_id == native_id, "Pi native session changed");
        anyhow::ensure!(input.cwd.is_absolute(), "Pi cwd must be absolute");
        crate::chat::config::validate_path_components(&input.cwd)?;
        Ok(input)
    }
}

/// Extension lifecycle failures stay out of model context; diagnostics are private.
pub fn run_hook(event: &str, name: Option<String>) -> i32 {
    #[cfg(target_os = "linux")]
    {
        let deadline = std::time::Instant::now() + crate::chat::delivery::HOOK_DEADLINE;
        let mut stage = "input";
        let result = (|| -> anyhow::Result<()> {
            let bytes = super::hooks::read_input(deadline)?;
            let native_id = std::env::var("PI_SESSION_ID")?;
            let input = HookInput::parse(&bytes, event, &native_id)?;
            stage = "binding";
            let paths = crate::chat::config::Paths::discover()?;
            super::binding::pi_check(&paths, &input, name, deadline)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = super::hooks::record_error("pi", stage);
            return 1;
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (event, name);
        return 1;
    }
    0
}

#[cfg(target_os = "linux")]
fn accepted_ack(bytes: &[u8], expected: &str) -> bool {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ack {
        attempt: String,
        accepted: bool,
    }
    serde_json::from_slice::<Ack>(bytes).is_ok_and(|ack| ack.attempt == expected && ack.accepted)
}

#[cfg(target_os = "linux")]
fn read_ack(expected: &str, deadline: std::time::Instant) -> bool {
    let mut line = Vec::with_capacity(128);
    while line.len() < 512 {
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            return false;
        };
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
        if ready <= 0 {
            return false;
        }
        let mut byte = 0u8;
        let count = unsafe { libc::read(libc::STDIN_FILENO, (&mut byte as *mut u8).cast(), 1) };
        if count < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if count != 1 {
            return false;
        }
        if byte == b'\n' {
            return accepted_ack(&line, expected);
        }
        line.push(byte);
    }
    false
}

#[cfg(target_os = "linux")]
fn page_addresses_session(
    page: &serde_json::Value,
    session: &crate::chat::types::SessionRef,
) -> anyhow::Result<bool> {
    use crate::chat::types::{FeedEvent, Target};
    let events = page["events"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("invalid Pi bridge feed page"))?;
    for event in events {
        let parsed: FeedEvent = serde_json::from_value(event["event"].clone())?;
        if let FeedEvent::Message { message } = parsed {
            if message.draft.to.contains(&Target::Agent(session.clone())) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(target_os = "linux")]
fn write_bridge_line(fd: i32, line: &str, deadline: std::time::Instant) -> anyhow::Result<()> {
    anyhow::ensure!(line.len() <= 32_768, "Pi bridge line exceeds bound");
    let previous = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    anyhow::ensure!(previous >= 0, "Pi bridge output is unavailable");
    anyhow::ensure!(
        unsafe { libc::fcntl(fd, libc::F_SETFL, previous | libc::O_NONBLOCK) } == 0,
        "Pi bridge output is unavailable"
    );
    struct RestoreFlags(i32, i32);
    impl Drop for RestoreFlags {
        fn drop(&mut self) {
            unsafe { libc::fcntl(self.0, libc::F_SETFL, self.1) };
        }
    }
    let _restore = RestoreFlags(fd, previous);
    let mut bytes = line.as_bytes();
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or_else(|| anyhow::anyhow!("Pi bridge output deadline expired"))?;
        let mut count =
            unsafe { libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL) };
        if count < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOTSOCK) {
            count = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        }
        if count > 0 {
            bytes = &bytes[count as usize..];
            continue;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(error.into());
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let ready = unsafe {
            libc::poll(
                &mut poll,
                1,
                remaining.as_millis().min(i32::MAX as u128) as i32,
            )
        };
        anyhow::ensure!(ready > 0, "Pi bridge output deadline expired");
        anyhow::ensure!(
            poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
            "Pi bridge output is unavailable"
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn bridge() -> anyhow::Result<()> {
    use crate::chat::{config::Paths, protocol, types::Handoff};
    use std::time::{Duration, Instant};
    let paths = Paths::discover()?;
    let participant =
        super::binding::Participant::current(&paths, Instant::now() + Duration::from_secs(6))?;
    let mut subscription = participant.subscribe_pi()?;
    loop {
        let response = protocol::read_frame(&mut subscription)?;
        anyhow::ensure!(
            response["version"] == 1 && response["ok"] == true,
            "Pi bridge subscription failed"
        );
        if !page_addresses_session(&response["data"], participant.session())? {
            continue;
        }
        while let Some(attempt) = participant.claim_pi()? {
            let line = serde_json::to_string(&serde_json::json!({"attempt": attempt}))? + "\n";
            let submitted = write_bridge_line(
                libc::STDOUT_FILENO,
                &line,
                Instant::now() + Duration::from_millis(500),
            );
            let confirmed = submitted.is_ok()
                && read_ack(&attempt.id, Instant::now() + Duration::from_millis(750));
            let outcome = if confirmed {
                Handoff::Accepted {
                    receipt: format!("Pi sendMessage returned for attempt {}", attempt.id),
                }
            } else {
                Handoff::Unknown {
                    reason: "Pi extension handoff did not confirm native queuing".into(),
                }
            };
            participant.finish_pi(&attempt.id, outcome)?;
            submitted?;
        }
    }
}

/// Stays silent until a directed arrival. The extension owns this child and
/// stops it before ending its native session.
pub fn run_bridge() -> anyhow::Result<i32> {
    #[cfg(target_os = "linux")]
    {
        bridge()?;
        Ok(0)
    }
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("Pi Chat bridge requires a Linux native binding")
}

#[cfg(test)]
mod tests {
    use super::HookInput;

    #[test]
    fn pi_lifecycle_requires_current_native_session_and_exact_event() {
        let input = br#"{"session_id":"11111111-1111-4111-8111-111111111111","cwd":"/tmp/owned","hook_event_name":"SessionStart"}"#;
        assert!(HookInput::parse(
            input,
            "SessionStart",
            "11111111-1111-4111-8111-111111111111"
        )
        .is_ok());
        assert!(
            HookInput::parse(input, "SessionEnd", "11111111-1111-4111-8111-111111111111").is_err()
        );
        assert!(HookInput::parse(
            input,
            "SessionStart",
            "22222222-2222-4222-8222-222222222222"
        )
        .is_err());
    }

    #[test]
    fn bridge_ack_must_match_the_claimed_attempt() {
        assert!(super::accepted_ack(
            br#"{"attempt":"11111111-1111-4111-8111-111111111111","accepted":true}"#,
            "11111111-1111-4111-8111-111111111111",
        ));
        assert!(!super::accepted_ack(
            br#"{"attempt":"22222222-2222-4222-8222-222222222222","accepted":true}"#,
            "11111111-1111-4111-8111-111111111111",
        ));
        assert!(!super::accepted_ack(
            br#"{"attempt":"11111111-1111-4111-8111-111111111111","accepted":false}"#,
            "11111111-1111-4111-8111-111111111111",
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bridge_output_supports_node_style_socketpair_stdout() {
        use std::{
            io::Read,
            os::fd::AsRawFd,
            os::unix::net::UnixStream,
            time::{Duration, Instant},
        };
        let (writer, mut reader) = UnixStream::pair().unwrap();
        super::write_bridge_line(
            writer.as_raw_fd(),
            "{\"ok\":true}\n",
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        let mut bytes = [0u8; 12];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"{\"ok\":true}\n");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bridge_ignores_unaddressed_feed_events() {
        use crate::chat::types::{Actor, Draft, FeedEvent, Message, SessionRef, Target};
        let wanted = SessionRef {
            machine: "11111111-1111-4111-8111-111111111111".into(),
            incarnation: "22222222-2222-4222-8222-222222222222".into(),
        };
        let other = SessionRef {
            machine: wanted.machine.clone(),
            incarnation: "33333333-3333-4333-8333-333333333333".into(),
        };
        let mut message = Message {
            id: "44444444-4444-4444-8444-444444444444".into(),
            sender: Actor::Agent(other.clone()),
            sender_seq: 1,
            created_at: "2026-09-20T00:00:00Z".into(),
            draft: Draft {
                key: "one".into(),
                project: "55555555-5555-4555-8555-555555555555".into(),
                to: vec![Target::Agent(other)],
                body: "body".into(),
                reply_to: None,
            },
        };
        let page = |message: &Message| serde_json::json!({"events": [{"event": FeedEvent::Message { message: message.clone() }}]});
        assert!(!super::page_addresses_session(&page(&message), &wanted).unwrap());
        message.draft.to.push(Target::Agent(wanted.clone()));
        assert!(super::page_addresses_session(&page(&message), &wanted).unwrap());
    }
}
