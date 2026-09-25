//! Drives a `round_based` state machine over a [`Transport`].
//!
//! The state machine tells us when it has a message to send, when it needs
//! one more message, and when it is done. Messages for other sessions that
//! arrive in the meantime are stashed in the [`Mailbox`] so that a fast
//! party starting the next protocol does not confuse a slow one.

use crate::transport::{Transport, TransportError, WireMessage};
use round_based::{
    state_machine::{ProceedResult, StateMachine},
    Incoming, MessageDestination, MessageType,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("message codec: {0}")]
    Codec(String),
    #[error("state machine: {0}")]
    Execution(String),
    #[error("protocol did not finish within {0:?}")]
    Deadline(Duration),
}

/// Per-transport inbox with a stash for messages of sessions we are not
/// running yet.
pub struct Mailbox<'a> {
    transport: &'a dyn Transport,
    stash: HashMap<String, VecDeque<WireMessage>>,
    /// Stashed messages are dropped beyond this many per session.
    stash_cap: usize,
}

impl<'a> Mailbox<'a> {
    pub fn new(transport: &'a dyn Transport) -> Self {
        Self {
            transport,
            stash: HashMap::new(),
            stash_cap: 4096,
        }
    }

    pub fn transport(&self) -> &'a dyn Transport {
        self.transport
    }

    /// Next message for `session`, waiting at most `timeout`. Messages of
    /// other sessions are stashed for later.
    pub fn recv_session(
        &mut self,
        session: &str,
        timeout: Duration,
    ) -> Result<WireMessage, TransportError> {
        if let Some(m) = self.stash.get_mut(session).and_then(VecDeque::pop_front) {
            return Ok(m);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(TransportError::Timeout);
            }
            let m = self.transport.recv(deadline - now)?;
            if m.session == session {
                return Ok(m);
            }
            let q = self.stash.entry(m.session.clone()).or_default();
            if q.len() < self.stash_cap {
                q.push_back(m);
            } else {
                tracing::warn!(session = %m.session, "stash full, dropping message");
            }
        }
    }

    /// Put a message back at the front of its session queue.
    pub fn unread(&mut self, m: WireMessage) {
        self.stash
            .entry(m.session.clone())
            .or_default()
            .push_front(m);
    }

    /// Whether any message is stashed for `session`.
    pub fn has_stashed(&self, session: &str) -> bool {
        self.stash.get(session).is_some_and(|q| !q.is_empty())
    }
}

/// Run `sm` to completion. `parties[local]` is the transport index of the
/// party the state machine knows as `local` (keygen uses the identity map;
/// signing uses the signer subset).
pub fn run<S>(
    sm: &mut S,
    mailbox: &mut Mailbox<'_>,
    session: &str,
    parties: &[u16],
    timeout: Duration,
) -> Result<S::Output, ProtocolError>
where
    S: StateMachine,
    S::Msg: Serialize + DeserializeOwned,
{
    let me = mailbox.transport().my_index();
    let deadline = Instant::now() + timeout;
    let mut next_id: u64 = 0;
    loop {
        if Instant::now() >= deadline {
            return Err(ProtocolError::Deadline(timeout));
        }
        match sm.proceed() {
            ProceedResult::SendMsg(out) => {
                let body = serde_json::to_vec(&out.msg)
                    .map_err(|e| ProtocolError::Codec(e.to_string()))?;
                let transport = mailbox.transport();
                match out.recipient {
                    MessageDestination::AllParties if parties.len() == transport.n() as usize => {
                        transport.send(&WireMessage::broadcast(session, me, body))?;
                    }
                    MessageDestination::AllParties => {
                        // Broadcast inside a subset: one delivery per member.
                        for g in parties.iter().filter(|g| **g != me) {
                            transport.send(&WireMessage {
                                session: session.to_string(),
                                from: me,
                                to: Some(*g),
                                broadcast: true,
                                body: body.clone(),
                            })?;
                        }
                    }
                    MessageDestination::OneParty(local) => {
                        let g = *parties
                            .get(local as usize)
                            .ok_or(TransportError::UnknownParty(local))?;
                        transport.send(&WireMessage::p2p(session, me, g, body))?;
                    }
                }
            }
            ProceedResult::NeedsOneMoreMessage => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let m = mailbox.recv_session(session, remaining)?;
                let Some(sender) = parties.iter().position(|g| *g == m.from) else {
                    tracing::warn!(
                        from = m.from,
                        session,
                        "message from a party outside this session dropped"
                    );
                    continue;
                };
                let msg: S::Msg = match serde_json::from_slice(&m.body) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(from = m.from, session, error = %e, "undecodable message dropped");
                        continue;
                    }
                };
                let msg_type = if m.broadcast {
                    MessageType::Broadcast
                } else {
                    MessageType::P2P
                };
                let incoming = Incoming {
                    id: next_id,
                    sender: sender as u16,
                    msg_type,
                    msg,
                };
                next_id += 1;
                if sm.received_msg(incoming).is_err() {
                    // The machine already holds an unconsumed message; try again later.
                    mailbox.unread(m);
                    next_id -= 1;
                }
            }
            ProceedResult::Yielded => continue,
            ProceedResult::Output(o) => return Ok(o),
            ProceedResult::Error(e) => return Err(ProtocolError::Execution(e.to_string())),
        }
    }
}
