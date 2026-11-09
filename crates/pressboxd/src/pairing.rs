// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: AGPL-3.0-only

//! Per-device tokens and pairing by short code.
//!
//! A new client calls `POST /api/v1/pair` with a device name and gets a short code
//! (shown to the user) plus a secret poll id. Someone approves the code with
//! `pressboxctl approve` or from an already-paired client. The new client's
//! next `GET /api/v1/pair/{poll_id}` then returns its own token, exactly once.
//!
//! Only SHA-256 hashes of device tokens are stored. Pending requests live in
//! memory and expire after ten minutes.

use std::{collections::HashMap, sync::Mutex};

use anyhow::Result;
use pressbox_api::{Approved, NewDevice, PairApproved, PairStart, PendingPairing};
use sha2::{Digest, Sha256};

use crate::db::{AuthUser, Db, hex};

/// How long a pairing request stays valid.
pub const TTL: i64 = 10 * 60;
/// Most pairing requests waiting at once; more are refused.
pub const MAX_PENDING: usize = 50;
/// Most pairing requests waiting from one client address, so nobody can
/// fill the queue and lock out everyone else.
pub const MAX_PER_CLIENT: usize = 3;
/// No 0/O or 1/I, so codes survive being read aloud or typed on a phone.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    bytes
}

/// A new random API token.
pub fn new_token() -> String {
    hex(&random_bytes::<32>())
}

/// What is stored for a token: its SHA-256, hex encoded.
pub fn hash_token(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// A code like `K7W-4QX`.
fn new_code() -> String {
    let bytes = random_bytes::<6>();
    let chars: String = bytes
        .iter()
        .map(|b| CODE_ALPHABET[usize::from(*b) % CODE_ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &chars[..3], &chars[3..])
}

/// Codes are compared without case, spaces or dashes.
fn normalize(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Checks and tidies a device name: 1–40 characters of letters, digits,
/// `-`, `_` and `.`, with spaces turned into `-`.
pub fn device_name(name: &str) -> Option<String> {
    let name: String = name.trim().replace(char::is_whitespace, "-");
    let valid = (1..=40).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    valid.then_some(name)
}

/// Creates a device for a user and returns its token.
pub fn create_device(db: &Db, user: &AuthUser, name: &str, now: i64) -> Result<NewDevice> {
    let token = new_token();
    let name = db.add_device(user.id, name, &hash_token(&token), now)?;
    Ok(NewDevice {
        name,
        user: user.name.clone(),
        token,
    })
}

struct Pending {
    code: String,
    name: String,
    /// The requesting client's address, for the per-client limit.
    client: String,
    poll_id: String,
    requested: i64,
    approved: Option<PairApproved>,
}

pub enum PollState {
    Pending,
    Approved(PairApproved),
    /// Unknown, expired, denied, or already collected.
    Gone,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    InvalidName,
    TooMany,
    TooManyFromClient,
}

#[derive(Default)]
pub struct Pairings {
    pending: Mutex<HashMap<String, Pending>>,
}

impl Pairings {
    fn lock(&self, now: i64) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|_, p| now - p.requested < TTL);
        pending
    }

    pub fn start(&self, name: &str, client: &str, now: i64) -> Result<PairStart, StartError> {
        let name = device_name(name).ok_or(StartError::InvalidName)?;
        let mut pending = self.lock(now);
        let from_client = pending
            .values()
            .filter(|p| p.approved.is_none() && p.client == client)
            .count();
        if from_client >= MAX_PER_CLIENT {
            return Err(StartError::TooManyFromClient);
        }
        if pending.len() >= MAX_PENDING {
            return Err(StartError::TooMany);
        }
        let code = loop {
            let code = new_code();
            if !pending.contains_key(&normalize(&code)) {
                break code;
            }
        };
        let poll_id = new_token();
        pending.insert(
            normalize(&code),
            Pending {
                code: code.clone(),
                name,
                client: client.to_owned(),
                poll_id: poll_id.clone(),
                requested: now,
                approved: None,
            },
        );
        Ok(PairStart {
            code,
            poll_id,
            expires_in: TTL.unsigned_abs(),
        })
    }

    pub fn poll(&self, poll_id: &str, now: i64) -> PollState {
        let mut pending = self.lock(now);
        let Some(key) = pending
            .iter()
            .find(|(_, p)| p.poll_id == poll_id)
            .map(|(k, _)| k.clone())
        else {
            return PollState::Gone;
        };
        if pending[&key].approved.is_none() {
            return PollState::Pending;
        }
        // Approved: hand the token over once, and forget the request.
        pending
            .remove(&key)
            .and_then(|p| p.approved)
            .map_or(PollState::Gone, PollState::Approved)
    }

    pub fn list(&self, now: i64) -> Vec<PendingPairing> {
        let mut list: Vec<_> = self
            .lock(now)
            .values()
            .filter(|p| p.approved.is_none())
            .map(|p| PendingPairing {
                code: p.code.clone(),
                name: p.name.clone(),
                requested: p.requested,
            })
            .collect();
        list.sort_by_key(|p| p.requested);
        list
    }

    /// Creates the device for a pending code, for `user`. `None` if there is
    /// no such code.
    pub fn approve(
        &self,
        code: &str,
        db: &Db,
        user: &AuthUser,
        now: i64,
    ) -> Result<Option<Approved>> {
        let mut pending = self.lock(now);
        let Some(p) = pending
            .get_mut(&normalize(code))
            .filter(|p| p.approved.is_none())
        else {
            return Ok(None);
        };
        let device = create_device(db, user, &p.name, now)?;
        p.approved = Some(PairApproved {
            token: device.token,
            name: device.name.clone(),
        });
        Ok(Some(Approved {
            name: device.name,
            user: device.user,
        }))
    }

    /// Returns false if there is no such pending code.
    pub fn deny(&self, code: &str, now: i64) -> bool {
        let mut pending = self.lock(now);
        let key = normalize(code);
        if pending.get(&key).is_some_and(|p| p.approved.is_none()) {
            pending.remove(&key);
            true
        } else {
            false
        }
    }

    pub fn count(&self, now: i64) -> usize {
        self.lock(now)
            .values()
            .filter(|p| p.approved.is_none())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_codes() {
        let token = new_token();
        assert_eq!(token.len(), 64);
        assert_ne!(token, new_token());
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let code = new_code();
        assert_eq!(code.len(), 7);
        assert_eq!(&code[3..4], "-");
        assert!(
            code.chars()
                .all(|c| c == '-' || CODE_ALPHABET.contains(&(c as u8)))
        );
        assert_eq!(normalize(" k7w-4qx "), "K7W4QX");
    }

    #[test]
    fn device_names() {
        assert_eq!(
            device_name(" Chris iPhone ").as_deref(),
            Some("Chris-iPhone")
        );
        assert_eq!(device_name("laptop.home").as_deref(), Some("laptop.home"));
        assert_eq!(device_name(""), None);
        assert_eq!(device_name("a/b"), None);
        assert_eq!(device_name(&"x".repeat(41)), None);
    }

    fn db_with_user() -> (Db, AuthUser) {
        let mut db = Db::open_in_memory().unwrap();
        db.sync(&crate::config::Config::parse("").unwrap()).unwrap();
        let user = db.only_user().unwrap().unwrap();
        (db, user)
    }

    #[test]
    fn pairing_flow() {
        let (db, user) = db_with_user();
        let pairings = Pairings::default();
        let start = pairings.start("laptop", "10.0.0.1", 0).unwrap();
        assert!(matches!(
            pairings.poll(&start.poll_id, 1),
            PollState::Pending
        ));
        assert_eq!(pairings.list(1)[0].name, "laptop");

        // Approving with a sloppy code works; the device gets a token once.
        let code = start.code.to_lowercase().replace('-', " ");
        let approved = pairings.approve(&code, &db, &user, 2).unwrap().unwrap();
        assert_eq!(
            (approved.name.as_str(), approved.user.as_str()),
            ("laptop", "default")
        );
        assert!(pairings.list(2).is_empty());
        assert!(
            pairings
                .approve(&start.code, &db, &user, 2)
                .unwrap()
                .is_none(),
            "only once"
        );
        let PollState::Approved(got) = pairings.poll(&start.poll_id, 3) else {
            panic!("expected a token");
        };
        let who = db
            .authenticate(&hash_token(&got.token), 4)
            .unwrap()
            .unwrap();
        assert_eq!(who.device.as_deref(), Some("laptop"));
        assert!(
            matches!(pairings.poll(&start.poll_id, 4), PollState::Gone),
            "collected"
        );

        // Denied and expired requests are gone.
        let denied = pairings.start("phone", "10.0.0.1", 10).unwrap();
        assert!(pairings.deny(&denied.code, 11));
        assert!(matches!(
            pairings.poll(&denied.poll_id, 12),
            PollState::Gone
        ));
        let expired = pairings.start("tablet", "10.0.0.1", 100).unwrap();
        assert!(matches!(
            pairings.poll(&expired.poll_id, 100 + TTL),
            PollState::Gone
        ));
        assert!(
            pairings
                .approve(&expired.code, &db, &user, 100 + TTL)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn limits() {
        let pairings = Pairings::default();
        assert_eq!(
            pairings.start("bad/name", "a", 0).unwrap_err(),
            StartError::InvalidName
        );
        // One client gets a few; others are unaffected.
        for _ in 0..MAX_PER_CLIENT {
            pairings.start("d", "attacker", 0).unwrap();
        }
        assert_eq!(
            pairings.start("d", "attacker", 0).unwrap_err(),
            StartError::TooManyFromClient
        );
        assert!(pairings.start("d", "someone else", 0).is_ok());
        // The global cap still holds against many addresses.
        let mut n = 0;
        while pairings.count(0) < MAX_PENDING {
            pairings.start("d", &format!("10.0.{n}.1"), 0).unwrap();
            n += 1;
        }
        assert_eq!(
            pairings.start("d", "fresh", 0).unwrap_err(),
            StartError::TooMany
        );
        assert!(
            pairings.start("d", "attacker", TTL).is_ok(),
            "old requests expire"
        );
    }
}
