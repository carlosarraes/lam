#[cfg(target_os = "linux")]
mod binding;
#[cfg(all(test, target_os = "linux"))]
pub(in crate::chat) use binding::{
    install_test_binding, ProcessEvidence, BACKEND_SOCKET_TEST_LOCK,
};
#[cfg(target_os = "linux")]
pub(super) use binding::{NativeBindings, Participant};
#[cfg(target_os = "linux")]
pub(in crate::chat) use codex::reserve_queue_epoch as reserve_codex_queue_epoch;
#[cfg(target_os = "linux")]
pub(in crate::chat) use codex::submit_queue_owned;
pub mod codex;

pub trait Adapter {
    fn validate(&self, session: &crate::chat::types::SessionRef) -> anyhow::Result<()>;
    fn submit(&mut self, attempt: &crate::chat::types::Attempt) -> crate::chat::types::Handoff;
    fn reconcile(&mut self, attempt_id: &str) -> crate::chat::types::Handoff;
}
