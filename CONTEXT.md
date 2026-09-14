# LAM

LAM separates requests for human attention from addressed coordination messages. This glossary includes the proposed Chat vocabulary; it does not imply Chat is implemented.

## Language

**Request**: An item requiring Carlos's input or decision.
_Avoid_: Chat message, notification

**FYI**: An informational item for Carlos that does not require a decision.
_Avoid_: Request

**Article**: A saved report for reading, with explicit read or unread state.
_Avoid_: Chat attachment

**Chat message**: An attributed coordination message addressed to specific participants. It is not a human approval request.
_Avoid_: Request, FYI

**Participant**: An addressable agent session or the human composing a message.
_Avoid_: Model, pane

**Session incarnation**: One registered lifetime of an agent session. A restarted agent with the same display name is a different incarnation.
_Avoid_: Name, conversation history

**Inbox**: The messages addressed to a participant, including retained messages available for recovery or later reading.
_Avoid_: Shared room

**Observer feed**: Carlos's view of coordination messages and their delivery receipts. Seeing a message here does not make every agent its recipient.
_Avoid_: Broadcast

**Delivery receipt**: Evidence of a particular delivery step for one recipient. It does not establish that the model understood a message or acted on it.
_Avoid_: Read receipt, task completion
