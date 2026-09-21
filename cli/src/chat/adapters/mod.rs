#[cfg(any(target_os = "linux", target_os = "macos"))]
mod binding;
#[cfg(all(test, target_os = "macos"))]
pub(in crate::chat) use binding::install_test_relay_binding;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(in crate::chat) use binding::registration_live;
#[cfg(all(test, target_os = "linux"))]
pub(in crate::chat) use binding::{
    install_test_binding, ProcessEvidence, BACKEND_SOCKET_TEST_LOCK,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) use binding::{NativeBindings, Participant};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(in crate::chat) use codex::reserve_queue_epoch as reserve_codex_queue_epoch;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(in crate::chat) use codex::submit_queue_owned;
pub mod claude;
pub mod codex;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod hooks;
#[cfg(target_os = "macos")]
mod macos_process;
pub mod pi;

pub trait Adapter {
    fn validate(&self, session: &crate::chat::types::SessionRef) -> anyhow::Result<()>;
    fn submit(&mut self, attempt: &crate::chat::types::Attempt) -> crate::chat::types::Handoff;
    fn reconcile(&mut self, attempt_id: &str) -> crate::chat::types::Handoff;
}
