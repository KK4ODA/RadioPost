// SPDX-License-Identifier: MIT OR Apache-2.0
//! How far a session has carried the messages in it, in bytes of their air forms (RP-2 §4): what a
//! program shows while a session runs.
//!
//! The engines count what they themselves do. A message sent counts as crossed once its pieces are
//! handed to the link, which a modem may still hold: RP-4's `queued` says how many bytes, and a
//! program that shows progress takes them off ([`Progress::sent_bytes`]). A message received
//! counts what has arrived. Nothing here changes what goes on the air.

use crate::crypto::ContentId;

/// One message crossing in a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crossing {
    /// The message.
    pub id: ContentId,
    /// Its air form's length.
    pub size: u64,
    /// What the receiving station held of it when the session began: a message resumed (RP-3
    /// §7) crosses only the rest.
    pub start: u64,
    /// How far it has crossed, counting `start`: handed to the link when sending, arrived when
    /// receiving.
    pub done: u64,
    /// The receiving station holds it whole: delivered, or received and checked.
    pub whole: bool,
    /// It will not cross in this session: not taken, or it failed its check.
    pub refused: bool,
}

impl Crossing {
    /// What it carries in this session.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.size.saturating_sub(self.start)
    }

    /// What of that has crossed.
    #[must_use]
    pub fn crossed(&self) -> u64 {
        if self.whole {
            self.bytes()
        } else {
            self.done.min(self.size).saturating_sub(self.start)
        }
    }
}

/// The messages of a session both ways.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// This station's messages, in the order they go.
    pub sending: Vec<Crossing>,
    /// The other station's, as far as they are known: listed, offered, or arriving.
    pub receiving: Vec<Crossing>,
}

fn entry(list: &mut Vec<Crossing>, id: ContentId, size: u64, start: u64) -> &mut Crossing {
    if let Some(at) = list.iter().position(|c| c.id == id) {
        return &mut list[at];
    }
    list.push(Crossing {
        id,
        size,
        start,
        done: start,
        whole: false,
        refused: false,
    });
    list.last_mut().expect("just pushed")
}

impl Progress {
    /// A message this station sends: `size` bytes, of which the other station holds `start`.
    pub fn will_send(&mut self, id: ContentId, size: u64, start: u64) {
        entry(&mut self.sending, id, size, start);
    }

    /// Its pieces from `start` on are handed to the link.
    pub fn handed(&mut self, id: ContentId) {
        if let Some(c) = self.sending.iter_mut().find(|c| c.id == id) {
            c.done = c.size;
        }
    }

    /// The other station holds it (`RECEIPT`), or will not take it.
    pub fn answered(&mut self, id: ContentId, taken: bool) {
        if let Some(c) = self.sending.iter_mut().find(|c| c.id == id) {
            c.whole = taken;
            c.refused = !taken;
        }
    }

    /// A message the other station will send: listed, offered, or held in part (`start`).
    pub fn will_receive(&mut self, id: ContentId, size: u64, start: u64) {
        entry(&mut self.receiving, id, size, start);
    }

    /// A piece arrived: `len` bytes at `offset` of an air form `total` bytes long.
    pub fn arrived(&mut self, id: ContentId, offset: u64, len: u64, total: u64) {
        let c = entry(&mut self.receiving, id, total, offset);
        c.size = total;
        c.done = c.done.max(offset.saturating_add(len));
    }

    /// The other station's turn ended: what it was expected to send and sent none of will not
    /// come in this one.
    pub fn turn_ended(&mut self) {
        for c in &mut self.receiving {
            if !c.whole && !c.refused && c.done <= c.start {
                c.refused = true;
            }
        }
    }

    /// It arrived whole and checked, or failed.
    pub fn received(&mut self, id: ContentId, ok: bool) {
        if let Some(c) = self.receiving.iter_mut().find(|c| c.id == id) {
            c.whole = ok;
            c.refused = !ok;
        }
    }

    /// The bytes this station sends in the session, and of them those the other station has,
    /// taking off `queued`, what the modem still holds of what was handed to it (RP-4 `queued`).
    #[must_use]
    pub fn sent_bytes(&self, queued: u64) -> (u64, u64) {
        let going = self.sending.iter().filter(|c| !c.refused);
        let total = going.clone().map(Crossing::bytes).sum();
        let whole: u64 = going.clone().filter(|c| c.whole).map(Crossing::bytes).sum();
        let in_flight: u64 = going.filter(|c| !c.whole).map(Crossing::crossed).sum();
        (total, (whole + in_flight.saturating_sub(queued)).min(total))
    }

    /// The bytes the other station sends, as far as known, and of them those that arrived.
    #[must_use]
    pub fn received_bytes(&self) -> (u64, u64) {
        let coming = self.receiving.iter().filter(|c| !c.refused);
        (
            coming.clone().map(Crossing::bytes).sum(),
            coming.map(Crossing::crossed).sum(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Progress;
    use crate::crypto::ContentId;

    fn id(n: u8) -> ContentId {
        ContentId::from_slice(&[n; 12]).expect("an ID")
    }

    /// Sending counts what the modem has passed on; receiving what arrived; a message resumed
    /// counts only the rest; a refused one leaves the totals.
    #[test]
    fn what_has_crossed() {
        let mut p = Progress::default();
        p.will_send(id(1), 1000, 0);
        p.will_send(id(2), 600, 200);
        p.will_send(id(3), 50, 0);
        assert_eq!(p.sent_bytes(0), (1450, 0), "nothing handed yet");
        p.handed(id(1));
        p.handed(id(2));
        assert_eq!(p.sent_bytes(1400), (1450, 0), "all still at the modem");
        assert_eq!(p.sent_bytes(400), (1450, 1000));
        p.answered(id(1), true);
        p.answered(id(3), false);
        assert_eq!(
            p.sent_bytes(400),
            (1400, 1000),
            "the refused one leaves the total"
        );
        assert_eq!(p.sent_bytes(0), (1400, 1400));

        p.will_receive(id(4), 900, 0);
        p.arrived(id(4), 0, 300, 900);
        p.arrived(id(5), 100, 100, 400);
        assert_eq!(p.received_bytes(), (900 + 300, 300 + 100));
        p.arrived(id(4), 300, 600, 900);
        p.received(id(4), true);
        p.received(id(5), false);
        assert_eq!(p.received_bytes(), (900, 900));
    }
}
