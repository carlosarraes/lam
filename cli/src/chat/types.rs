#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SessionRef {
    pub machine: String,
    pub incarnation: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Actor {
    Agent(SessionRef),
    Human { machine: String },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Target {
    Agent(SessionRef),
    Human { machine: String },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Draft {
    pub key: String,
    pub project: String,
    pub to: Vec<Target>,
    pub body: String,
    pub reply_to: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub id: String,
    pub sender: Actor,
    pub sender_seq: u64,
    pub created_at: String,
    pub draft: Draft,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub inline_bytes: usize,
    pub batch_bytes: usize,
}
