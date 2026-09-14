#[cfg(target_os = "linux")]
mod binding;
#[cfg(target_os = "linux")]
pub(super) use binding::{NativeBindings, Participant};
pub mod codex;

pub trait Adapter {
    fn validate(&self, session: &crate::chat::types::SessionRef) -> anyhow::Result<()>;
    fn submit(&mut self, attempt: &crate::chat::types::Attempt) -> crate::chat::types::Handoff;
    fn reconcile(&mut self, attempt_id: &str) -> crate::chat::types::Handoff;
}
