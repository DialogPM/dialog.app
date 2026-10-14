//! Key transparency verification (docs/KEY-TRANSPARENCY.md): Merkle proofs (RFC 6962 hashing,
//! RFC 9162 verification algorithms), signed tree heads and the decisions taken on them.
//!
//! Pure functions only; the engine (engine/keytrans.rs) fetches the data and acts on the
//! outcomes. The encodings mirror server-rs/src/keytrans.rs.

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::crypto::{b64, unb64};

pub type Hash = [u8; 32];

pub const LEAF_VERSION: u8 = 1;
pub const STH_CONTEXT: &[u8] = b"chatapp/kt/sth/v1";

/// Leaf data: version || account UUID || key length || identity key || created_at (u64 BE).
pub fn leaf_data(account_id: &str, identity_key: &[u8], created_at: i64) -> Option<Vec<u8>> {
    let id = uuid::Uuid::parse_str(account_id).ok()?;
    let len = u8::try_from(identity_key.len()).ok()?;
    let mut d = Vec::with_capacity(1 + 16 + 1 + identity_key.len() + 8);
    d.push(LEAF_VERSION);
    d.extend_from_slice(id.as_bytes());
    d.push(len);
    d.extend_from_slice(identity_key);
    d.extend_from_slice(&(created_at as u64).to_be_bytes());
    Some(d)
}

pub fn leaf_hash(data: &[u8]) -> Hash {
    let mut h = Sha256::new();
    h.update([0u8]);
    h.update(data);
    h.finalize().into()
}

pub fn node_hash(l: &Hash, r: &Hash) -> Hash {
    let mut h = Sha256::new();
    h.update([1u8]);
    h.update(l);
    h.update(r);
    h.finalize().into()
}

pub fn empty_root() -> Hash {
    Sha256::digest([]).into()
}

/// RFC 9162 2.1.3.2: whether `path` proves leaf `index` in the tree of `size` leaves with `root`.
pub fn verify_inclusion(index: u64, size: u64, leaf: &Hash, path: &[Hash], root: &Hash) -> bool {
    if index >= size {
        return false;
    }
    let (mut f, mut s, mut r) = (index, size - 1, *leaf);
    for p in path {
        if s == 0 {
            return false;
        }
        if f & 1 == 1 || f == s {
            r = node_hash(p, &r);
            while f & 1 == 0 && f != 0 {
                f >>= 1;
                s >>= 1;
            }
        } else {
            r = node_hash(&r, p);
        }
        f >>= 1;
        s >>= 1;
    }
    s == 0 && &r == root
}

/// RFC 9162 2.1.4.2: whether `proof` shows the tree (`second`, size n) extends (`first`, size m).
/// Every tree extends the empty one.
pub fn verify_consistency(m: u64, n: u64, first: &Hash, second: &Hash, proof: &[Hash]) -> bool {
    if m == 0 {
        return proof.is_empty() && m <= n;
    }
    if m == n {
        return proof.is_empty() && first == second;
    }
    if m > n || proof.is_empty() {
        return false;
    }
    let mut p: Vec<Hash> = Vec::with_capacity(proof.len() + 1);
    if m.is_power_of_two() {
        p.push(*first);
    }
    p.extend_from_slice(proof);
    let (mut f, mut s) = (m - 1, n - 1);
    while f & 1 == 1 {
        f >>= 1;
        s >>= 1;
    }
    let (mut fr, mut sr) = (p[0], p[0]);
    for c in &p[1..] {
        if s == 0 {
            return false;
        }
        if f & 1 == 1 || f == s {
            fr = node_hash(c, &fr);
            sr = node_hash(c, &sr);
            while f & 1 == 0 && f != 0 {
                f >>= 1;
                s >>= 1;
            }
        } else {
            sr = node_hash(&sr, c);
        }
        f >>= 1;
        s >>= 1;
    }
    &fr == first && &sr == second && s == 0
}

/// The right edge of a Merkle tree over leaves [0, size): the roots of its perfect subtrees,
/// largest first. Enough to append leaves and compute the root (clients monitoring the log).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CompactRange {
    pub size: u64,
    /// Hex-encoded subtree roots.
    pub nodes: Vec<String>,
}

impl CompactRange {
    pub fn push(&mut self, leaf: Hash) {
        let mut h = leaf;
        let mut s = self.size;
        while s & 1 == 1 {
            let left = self.nodes.pop().and_then(|n| hex32(&n)).unwrap_or([0u8; 32]);
            h = node_hash(&left, &h);
            s >>= 1;
        }
        self.nodes.push(hex::encode(h));
        self.size += 1;
    }

    pub fn root(&self) -> Hash {
        let mut nodes = self.nodes.iter().rev().filter_map(|n| hex32(n));
        let Some(mut r) = nodes.next() else { return empty_root() };
        for n in nodes {
            r = node_hash(&n, &r);
        }
        r
    }
}

fn hex32(s: &str) -> Option<Hash> {
    hex::decode(s).ok()?.try_into().ok()
}

/// Decodes a base64 hash.
pub fn hash_b64(s: &str) -> Option<Hash> {
    unb64(s).ok()?.try_into().ok()
}

pub fn hashes_b64(v: &[String]) -> Option<Vec<Hash>> {
    v.iter().map(|s| hash_b64(s)).collect()
}

/// A signed tree head, as served by the server and gossiped between clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TreeHead {
    pub tree_size: u64,
    pub root_hash: String,
    pub timestamp: i64,
    pub signature: String,
}

pub fn sth_message(size: u64, root: &[u8], timestamp: i64) -> Vec<u8> {
    let mut m = Vec::with_capacity(STH_CONTEXT.len() + 48);
    m.extend_from_slice(STH_CONTEXT);
    m.extend_from_slice(&size.to_be_bytes());
    m.extend_from_slice(root);
    m.extend_from_slice(&(timestamp as u64).to_be_bytes());
    m
}

impl TreeHead {
    pub fn root(&self) -> Option<Hash> {
        hash_b64(&self.root_hash)
    }

    /// Whether the head is well-formed and signed by `key` (the pinned log key).
    pub fn verify(&self, key: &[u8; 32]) -> bool {
        let (Some(root), Ok(sig)) = (self.root(), unb64(&self.signature)) else { return false };
        let (Ok(vk), Ok(sig)) = (VerifyingKey::from_bytes(key), Signature::from_slice(&sig)) else { return false };
        vk.verify_strict(&sth_message(self.tree_size, &root, self.timestamp), &sig).is_ok()
    }

    /// Signs a head (tests, and the evil-server tests that play the server).
    pub fn sign(key: &ed25519_dalek::SigningKey, size: u64, root: &Hash, timestamp: i64) -> TreeHead {
        use ed25519_dalek::Signer;
        TreeHead {
            tree_size: size,
            root_hash: b64(root),
            timestamp,
            signature: b64(key.sign(&sth_message(size, root, timestamp)).to_bytes()),
        }
    }
}

/// A log entry as served by the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub index: u64,
    pub account_id: String,
    pub identity_key: String,
    pub created_at: i64,
}

impl Entry {
    pub fn hash(&self) -> Option<Hash> {
        Some(leaf_hash(&leaf_data(&self.account_id, &unb64(&self.identity_key).ok()?, self.created_at)?))
    }
}

/// Why a view of the log was refused. Everything but `Unavailable` is evidence that the server
/// (or the network path to it) lies.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The head is not signed by the pinned key.
    BadSignature,
    /// Two signed heads that cannot both be views of one append-only log.
    Fork { ours: TreeHead, theirs: TreeHead },
    /// The server went back to an older head than one it already showed us.
    Rollback,
    /// An inclusion proof does not verify.
    BadProof,
    /// The log entry is for another account or another key than the one we were given.
    KeyMismatch,
}

/// Accepts `new` as the latest head if it is signed and extends `ours` (proved by `proof`
/// from `ours.tree_size`). Returns the head to keep.
pub fn accept_head(key: &[u8; 32], ours: Option<&TreeHead>, new: &TreeHead, proof: Option<&[Hash]>) -> Result<TreeHead, Failure> {
    if !new.verify(key) {
        return Err(Failure::BadSignature);
    }
    let Some(ours) = ours else { return Ok(new.clone()) };
    let fork = || Failure::Fork { ours: ours.clone(), theirs: new.clone() };
    let (Some(r_ours), Some(r_new)) = (ours.root(), new.root()) else { return Err(fork()) };
    if new.tree_size < ours.tree_size {
        return Err(Failure::Rollback);
    }
    if new.tree_size == ours.tree_size {
        return if r_ours == r_new { Ok(if new.timestamp > ours.timestamp { new.clone() } else { ours.clone() }) } else { Err(fork()) };
    }
    if verify_consistency(ours.tree_size, new.tree_size, &r_ours, &r_new, proof.unwrap_or_default()) {
        Ok(new.clone())
    } else {
        Err(fork())
    }
}

/// Checks that `entry` is in the tree of `head` (already accepted) and is `account_id`'s entry
/// with `identity_key` (when given).
pub fn check_entry(head: &TreeHead, entry: &Entry, inclusion: &[Hash], account_id: &str, identity_key: Option<&[u8]>) -> Result<(), Failure> {
    let (Some(leaf), Some(root)) = (entry.hash(), head.root()) else { return Err(Failure::BadProof) };
    if !verify_inclusion(entry.index, head.tree_size, &leaf, inclusion, &root) {
        return Err(Failure::BadProof);
    }
    let same_account = uuid::Uuid::parse_str(&entry.account_id).ok() == uuid::Uuid::parse_str(account_id).ok();
    let same_key = identity_key.is_none_or(|k| unb64(&entry.identity_key).ok().as_deref() == Some(k));
    if same_account && same_key {
        Ok(())
    } else {
        Err(Failure::KeyMismatch)
    }
}

/// What to do with a head a contact gossiped to us.
#[derive(Debug, Clone, PartialEq)]
pub enum Gossip {
    /// Same log as ours (nothing to do).
    Consistent,
    /// Consistent and newer than ours: adopt it.
    Newer,
    /// Needs a consistency proof between these sizes (smaller, larger) from the server.
    NeedProof(u64, u64),
}

/// First step of comparing a gossiped head with ours (both signed and verified).
pub fn compare_gossip(ours: &TreeHead, theirs: &TreeHead) -> Result<Gossip, Failure> {
    let fork = || Failure::Fork { ours: ours.clone(), theirs: theirs.clone() };
    if theirs.tree_size == ours.tree_size {
        return if theirs.root() == ours.root() && theirs.root().is_some() { Ok(Gossip::Consistent) } else { Err(fork()) };
    }
    if theirs.tree_size == 0 || ours.tree_size == 0 {
        return Ok(if theirs.tree_size > ours.tree_size { Gossip::Newer } else { Gossip::Consistent });
    }
    Ok(Gossip::NeedProof(ours.tree_size.min(theirs.tree_size), ours.tree_size.max(theirs.tree_size)))
}

/// Second step: checks the server's consistency proof between the two heads.
pub fn finish_gossip(ours: &TreeHead, theirs: &TreeHead, proof: &[Hash]) -> Result<Gossip, Failure> {
    let fork = || Failure::Fork { ours: ours.clone(), theirs: theirs.clone() };
    let (Some(ro), Some(rt)) = (ours.root(), theirs.root()) else { return Err(fork()) };
    let (small, large) = if ours.tree_size <= theirs.tree_size { ((ours, ro), (theirs, rt)) } else { ((theirs, rt), (ours, ro)) };
    if !verify_consistency(small.0.tree_size, large.0.tree_size, &small.1, &large.1, proof) {
        return Err(fork());
    }
    Ok(if theirs.tree_size > ours.tree_size { Gossip::Newer } else { Gossip::Consistent })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A reference log (what an honest or an evil server computes), straight from RFC 6962.
    pub struct RefLog(pub Vec<Hash>);

    fn split(n: usize) -> usize {
        let mut k = 1;
        while k * 2 < n {
            k *= 2;
        }
        k
    }

    impl RefLog {
        pub fn mth(l: &[Hash]) -> Hash {
            match l.len() {
                0 => empty_root(),
                1 => l[0],
                n => {
                    let k = split(n);
                    node_hash(&Self::mth(&l[..k]), &Self::mth(&l[k..]))
                }
            }
        }
        pub fn root(&self, n: usize) -> Hash {
            Self::mth(&self.0[..n])
        }
        pub fn path(&self, m: usize, n: usize) -> Vec<Hash> {
            fn go(l: &[Hash], m: usize, out: &mut Vec<Hash>) {
                if l.len() <= 1 {
                    return;
                }
                let k = split(l.len());
                if m < k {
                    go(&l[..k], m, out);
                    out.push(RefLog::mth(&l[k..]));
                } else {
                    go(&l[k..], m - k, out);
                    out.push(RefLog::mth(&l[..k]));
                }
            }
            let mut out = Vec::new();
            go(&self.0[..n], m, &mut out);
            out
        }
        pub fn consistency(&self, m: usize, n: usize) -> Vec<Hash> {
            fn sub(l: &[Hash], m: usize, complete: bool, out: &mut Vec<Hash>) {
                if m == l.len() {
                    if !complete {
                        out.push(RefLog::mth(l));
                    }
                    return;
                }
                let k = split(l.len());
                if m <= k {
                    sub(&l[..k], m, complete, out);
                    out.push(RefLog::mth(&l[k..]));
                } else {
                    sub(&l[k..], m - k, false, out);
                    out.push(RefLog::mth(&l[..k]));
                }
            }
            let mut out = Vec::new();
            if 0 < m && m < n {
                sub(&self.0[..n], m, true, &mut out);
            }
            out
        }
    }

    fn leaves(n: usize, salt: u8) -> Vec<Hash> {
        (0..n).map(|i| leaf_hash(&[salt, i as u8, (i >> 8) as u8])).collect()
    }

    #[test]
    fn inclusion_proofs() {
        let log = RefLog(leaves(37, 0));
        for n in 1..=37 {
            let root = log.root(n);
            for m in 0..n {
                let path = log.path(m, n);
                assert!(verify_inclusion(m as u64, n as u64, &log.0[m], &path, &root), "{m}/{n}");
                // Adversarial: wrong root, wrong leaf, wrong index, truncated or extended path.
                assert!(!verify_inclusion(m as u64, n as u64, &log.0[m], &path, &[1u8; 32]));
                assert!(!verify_inclusion(m as u64, n as u64, &[2u8; 32], &path, &root));
                assert!(!verify_inclusion(n as u64, n as u64, &log.0[m], &path, &root));
                if !path.is_empty() {
                    assert!(!verify_inclusion(m as u64, n as u64, &log.0[m], &path[..path.len() - 1], &root), "truncated {m}/{n}");
                }
                let mut longer = path.clone();
                longer.push([3u8; 32]);
                assert!(!verify_inclusion(m as u64, n as u64, &log.0[m], &longer, &root), "extended {m}/{n}");
            }
        }
    }

    #[test]
    fn consistency_proofs() {
        let log = RefLog(leaves(37, 0));
        for n in 1..=37 {
            for m in 0..=n {
                let proof = log.consistency(m, n);
                let (r1, r2) = (log.root(m), log.root(n));
                assert!(verify_consistency(m as u64, n as u64, &r1, &r2, &proof), "{m}->{n}");
                if 0 < m && m < n {
                    assert!(!verify_consistency(m as u64, n as u64, &r1, &[1u8; 32], &proof), "wrong second root {m}->{n}");
                    assert!(!verify_consistency(m as u64, n as u64, &[1u8; 32], &r2, &proof), "wrong first root {m}->{n}");
                    assert!(!verify_consistency(m as u64, n as u64, &r1, &r2, &proof[..proof.len() - 1]), "truncated {m}->{n}");
                    assert!(!verify_consistency(m as u64, n as u64, &r1, &r2, &[]), "empty {m}->{n}");
                }
            }
        }
        // A rewritten history (same size, one leaf changed) is not consistent with the original.
        let mut forked = log.0.clone();
        forked[5] = [9u8; 32];
        let forked = RefLog(forked);
        for m in 6..=37 {
            for n in m..=37 {
                let p = forked.consistency(m, n);
                assert!(!verify_consistency(m as u64, n as u64, &log.root(m), &forked.root(n), &p), "{m}->{n}");
            }
        }
    }

    #[test]
    fn compact_range_tracks_the_root() {
        let log = RefLog(leaves(70, 4));
        let mut cr = CompactRange::default();
        assert_eq!(cr.root(), empty_root());
        for (i, l) in log.0.iter().enumerate() {
            cr.push(*l);
            assert_eq!(cr.root(), log.root(i + 1), "size {}", i + 1);
        }
        let back: CompactRange = serde_json::from_str(&serde_json::to_string(&cr).unwrap()).unwrap();
        assert_eq!(back, cr);
    }

    fn key() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[42u8; 32])
    }

    #[test]
    fn heads_and_entries() {
        let sk = key();
        let pk = sk.verifying_key().to_bytes();
        let acct = "6f1c2a52-6a87-4a3c-9b1f-3c6c1d5a2b10";
        let ik = [5u8; 33];
        let mut ls = leaves(10, 1);
        ls.push(leaf_hash(&leaf_data(acct, &ik, 1000).unwrap()));
        ls.extend(leaves(5, 2));
        let log = RefLog(ls);
        let h8 = TreeHead::sign(&sk, 8, &log.root(8), 1);
        let h16 = TreeHead::sign(&sk, 16, &log.root(16), 2);
        assert_eq!(accept_head(&pk, None, &h8, None), Ok(h8.clone()));
        let p = log.consistency(8, 16);
        assert_eq!(accept_head(&pk, Some(&h8), &h16, Some(&p)), Ok(h16.clone()));
        // Bad signature, rollback, missing proof.
        let mut bad = h16.clone();
        bad.tree_size = 15;
        assert_eq!(accept_head(&pk, Some(&h8), &bad, Some(&p)), Err(Failure::BadSignature));
        assert_eq!(accept_head(&pk, Some(&h16), &h8, None), Err(Failure::Rollback));
        assert!(matches!(accept_head(&pk, Some(&h8), &h16, None), Err(Failure::Fork { .. })));
        let other = ed25519_dalek::SigningKey::from_bytes(&[43u8; 32]);
        assert_eq!(accept_head(&pk, None, &TreeHead::sign(&other, 8, &log.root(8), 1), None), Err(Failure::BadSignature));

        let entry = Entry { index: 10, account_id: acct.into(), identity_key: b64(ik), created_at: 1000 };
        let path = log.path(10, 16);
        assert_eq!(check_entry(&h16, &entry, &path, acct, Some(&ik)), Ok(()));
        // The server hands out another key for the account: the entry it proves is not that key.
        assert_eq!(check_entry(&h16, &entry, &path, acct, Some(&[6u8; 33])), Err(Failure::KeyMismatch));
        // Or it forges an entry with the fake key: there is no valid inclusion proof for it.
        let forged = Entry { identity_key: b64([6u8; 33]), ..entry.clone() };
        assert_eq!(check_entry(&h16, &forged, &path, acct, Some(&[6u8; 33])), Err(Failure::BadProof));
        // A proof for another account's entry is not one for ours.
        assert_eq!(
            check_entry(&h16, &entry, &path, "00000000-0000-4000-8000-000000000000", Some(&ik)),
            Err(Failure::KeyMismatch)
        );
    }

    #[test]
    fn gossip_detects_a_split_view() {
        let sk = key();
        let honest = RefLog(leaves(20, 1));
        let mut evil = honest.0.clone();
        evil[12] = leaf_hash(b"the fake key for someone");
        let evil = RefLog(evil);
        let ours = TreeHead::sign(&sk, 20, &honest.root(20), 1);
        // Same size, different root: a split view, and the two signatures prove it.
        let theirs = TreeHead::sign(&sk, 20, &evil.root(20), 2);
        assert!(matches!(compare_gossip(&ours, &theirs), Err(Failure::Fork { .. })));
        // Different sizes: the server has to prove consistency, and it cannot.
        let theirs = TreeHead::sign(&sk, 17, &evil.root(17), 2);
        assert_eq!(compare_gossip(&ours, &theirs), Ok(Gossip::NeedProof(17, 20)));
        for proof in [honest.consistency(17, 20), evil.consistency(17, 20), vec![]] {
            assert!(matches!(finish_gossip(&ours, &theirs, &proof), Err(Failure::Fork { .. })));
        }
        // An honest older or newer head verifies.
        let older = TreeHead::sign(&sk, 13, &honest.root(13), 0);
        assert_eq!(finish_gossip(&ours, &older, &honest.consistency(13, 20)), Ok(Gossip::Consistent));
        let mut grown = honest.0.clone();
        grown.extend(leaves(4, 9));
        let grown = RefLog(grown);
        let newer = TreeHead::sign(&sk, 24, &grown.root(24), 3);
        assert_eq!(compare_gossip(&ours, &newer), Ok(Gossip::NeedProof(20, 24)));
        assert_eq!(finish_gossip(&ours, &newer, &grown.consistency(20, 24)), Ok(Gossip::Newer));
        assert_eq!(compare_gossip(&ours, &ours), Ok(Gossip::Consistent));
    }
}
