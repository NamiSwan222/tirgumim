//! Who owns which paid translation: the entitlement list the account server signs (Ed25519), checked against the
//! public keys built into the launcher, kept on disk for offline play until it expires, and the gate that install,
//! update and play go through. Free games never reach the gate. The contract is in ENTITLEMENTS.md.
//!
//! This is a check in the launcher, not the protection itself: a paid pack is only ever downloadable through a
//! short-lived download grant the server issues after checking the account (see `DownloadGrant`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

use super::installer::PackError;
use super::util;

/// `typ` of a signed entitlement list.
pub const ENTITLEMENTS_TYP: &str = "tirgumim.entitlements.v1";
/// `typ` of a signed download grant.
pub const GRANT_TYP: &str = "tirgumim.download.v1";
/// Clocks may differ this much between the PC and the server.
pub const CLOCK_SKEW_SECS: u64 = 300;
/// However long the server says, a list is never trusted offline longer than this after it was issued.
pub const MAX_OFFLINE_SECS: u64 = 14 * 86_400 + 3600;
/// A download grant is never accepted longer than this after it was issued.
pub const MAX_GRANT_SECS: u64 = 3600;

// ---------- keys and signatures ----------

/// The server's public keys, by key id (several during a key change).
#[derive(Clone, Default)]
pub struct Keys(Vec<(String, VerifyingKey)>);

impl Keys {
    /// `"k1:<base64url 32-byte key>,k2:<…>"`. Empty: no keys (paid games can't be verified).
    pub fn parse(spec: &str) -> Result<Keys, String> {
        let mut out = vec![];
        for part in spec.split([',', ';', '\n']).map(str::trim).filter(|p| !p.is_empty()) {
            let (kid, key) = part.split_once(':').ok_or_else(|| format!("Key needs kid:key: {part}"))?;
            let raw = B64.decode(key.trim().trim_end_matches('=')).map_err(|e| format!("Key {kid}: {e}"))?;
            let bytes: [u8; 32] = raw.try_into().map_err(|_| format!("Key {kid}: not 32 bytes"))?;
            let vk = VerifyingKey::from_bytes(&bytes).map_err(|e| format!("Key {kid}: {e}"))?;
            out.push((kid.trim().to_string(), vk));
        }
        Ok(Keys(out))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn get(&self, kid: &str) -> Option<&VerifyingKey> {
        self.0.iter().find(|(k, _)| k == kid).map(|(_, v)| v)
    }
}

/// What the server sends: base64url JSON bytes and an Ed25519 signature over exactly those bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub kid: String,
    pub payload: String,
    pub sig: String,
}

/// Checks the signature and returns the signed JSON bytes.
pub fn verify_envelope(keys: &Keys, env: &Envelope) -> Result<Vec<u8>, String> {
    let key = keys.get(&env.kid).ok_or_else(|| format!("Unknown signing key {}", env.kid))?;
    let payload = B64.decode(env.payload.trim_end_matches('=')).map_err(|_| "Bad payload encoding".to_string())?;
    let sig_bytes: [u8; 64] = B64.decode(env.sig.trim_end_matches('=')).ok().and_then(|s| s.try_into().ok()).ok_or("Bad signature encoding")?;
    key.verify_strict(&payload, &Signature::from_bytes(&sig_bytes)).map_err(|_| "Signature does not match".to_string())?;
    Ok(payload)
}

// ---------- the entitlement list ----------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Purchase,
    Grant,
    AdminApproved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EntStatus {
    Active,
    Revoked,
    Refunded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entitlement {
    pub game_id: String,
    pub source: Source,
    pub status: EntStatus,
    /// ISO 8601 (UTC).
    pub granted_at: String,
    #[serde(default)]
    pub order_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementDoc {
    pub typ: String,
    pub account_id: String,
    pub device_id: String,
    /// Unix seconds (server clock).
    pub issued_at: u64,
    pub expires_at: u64,
    pub entitlements: Vec<Entitlement>,
}

/// Verifies a signed entitlement list for this account and device. Times are checked by `Snapshot::evaluate`.
pub fn parse_doc(keys: &Keys, env: &Envelope, account_id: &str, device_id: &str) -> Result<EntitlementDoc, String> {
    let bytes = verify_envelope(keys, env)?;
    let doc: EntitlementDoc = serde_json::from_slice(&bytes).map_err(|e| format!("Bad entitlement list: {e}"))?;
    if doc.typ != ENTITLEMENTS_TYP {
        return Err(format!("Not an entitlement list: {}", doc.typ));
    }
    if doc.account_id != account_id {
        return Err("Entitlement list is for another account".into());
    }
    if doc.device_id != device_id {
        return Err("Entitlement list is for another device".into());
    }
    if doc.expires_at <= doc.issued_at {
        return Err("Entitlement list expires before it was issued".into());
    }
    Ok(doc)
}

// ---------- what the launcher knows right now ----------

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ListState {
    /// The launcher was built without an account server or keys.
    NotConfigured,
    SignedOut,
    /// Signed in, but no list yet (never reached the server).
    None,
    Valid,
    /// Offline longer than the list allows: connect to refresh.
    Expired,
    /// The PC's clock is earlier than a time the launcher has already seen.
    ClockChanged,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub state: ListState,
    pub issued_at: Option<u64>,
    pub expires_at: Option<u64>,
    /// Every entitlement in the list (also revoked and refunded ones, for the UI). Empty unless `state` is Valid.
    pub items: Vec<Entitlement>,
}

impl Snapshot {
    pub fn plain(state: ListState) -> Snapshot {
        Snapshot { state, issued_at: None, expires_at: None, items: vec![] }
    }

    /// `high_water`: the latest time the launcher has seen (its own clock or a server's `issuedAt`). A clock earlier
    /// than that by more than the allowed skew means someone turned it back to stretch an offline list.
    pub fn evaluate(doc: Option<&EntitlementDoc>, now: u64, high_water: u64) -> Snapshot {
        let Some(doc) = doc else { return Snapshot::plain(ListState::None) };
        let mut s = Snapshot { state: ListState::Valid, issued_at: Some(doc.issued_at), expires_at: Some(doc.expires_at), items: vec![] };
        if now + CLOCK_SKEW_SECS < high_water.max(doc.issued_at) {
            s.state = ListState::ClockChanged;
            return s;
        }
        let expires = doc.expires_at.min(doc.issued_at.saturating_add(MAX_OFFLINE_SECS));
        if now.max(high_water) >= expires {
            s.state = ListState::Expired;
            return s;
        }
        s.items = doc.entitlements.clone();
        s
    }

    pub fn owns(&self, game_id: &str) -> bool {
        self.state == ListState::Valid && self.items.iter().any(|e| e.game_id == game_id && e.status == EntStatus::Active)
    }

    /// What the game page shows: "free", "owned", "notOwned", "revoked", "refunded", "signedOut", "expired",
    /// "clockChanged", "checking" (signed in, no list yet) or "notConfigured".
    pub fn access(&self, paid: bool, game_id: &str) -> &'static str {
        if !paid {
            return "free";
        }
        match self.state {
            ListState::NotConfigured => "notConfigured",
            ListState::SignedOut => "signedOut",
            ListState::None => "checking",
            ListState::Expired => "expired",
            ListState::ClockChanged => "clockChanged",
            ListState::Valid if self.owns(game_id) => "owned",
            ListState::Valid => match self.items.iter().rev().find(|e| e.game_id == game_id).map(|e| e.status) {
                Some(EntStatus::Refunded) => "refunded",
                Some(EntStatus::Revoked) => "revoked",
                _ => "notOwned",
            },
        }
    }

    /// The gate for install, update and play. Free games always pass.
    pub fn check(&self, paid: bool, game_id: &str) -> Result<(), PackError> {
        if !paid {
            return Ok(());
        }
        let e = |code: &str, msg: &str| Err(PackError::new(code, msg));
        match self.access(true, game_id) {
            "owned" => Ok(()),
            "notConfigured" => e("ACCOUNTS_NOT_CONFIGURED", "This launcher has no account server; paid translations are not available"),
            "signedOut" => e("SIGN_IN_REQUIRED", "Sign in to use this translation"),
            "checking" => e("ENTITLEMENTS_UNAVAILABLE", "Could not check your translations with the server"),
            "expired" => e("ENTITLEMENTS_EXPIRED", "Connect to the internet once so the launcher can check your translations"),
            "clockChanged" => e("CLOCK_CHANGED", "The computer's clock was turned back; fix the date and time"),
            "refunded" => e("NOT_ENTITLED", "This purchase was refunded"),
            "revoked" => e("NOT_ENTITLED", "Access to this translation was removed"),
            _ => e("NOT_ENTITLED", "This translation is not in your account"),
        }
    }
}

// ---------- the cache on disk ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheFile {
    #[serde(default)]
    envelope: Option<Envelope>,
    /// The latest time seen (see `Snapshot::evaluate`). Never goes down.
    #[serde(default)]
    high_water: u64,
}

/// `entitlements.json` in the data folder: the last signed list and the latest time seen. It is signed, so editing
/// it only breaks it; it is checked again every time it is read.
pub struct Cache {
    file: PathBuf,
}

impl Cache {
    pub fn new(data_dir: &Path) -> Cache {
        Cache { file: data_dir.join("account").join("entitlements.json") }
    }

    fn read(&self) -> CacheFile {
        util::read_json(&self.file).unwrap_or_default()
    }

    /// Stores a list the server just sent (already verified by the caller). The server's time is the authority, so
    /// the high-water mark is reset to it: a clock that was once wrongly far ahead doesn't lock paid games forever.
    pub fn store(&self, env: &Envelope, doc: &EntitlementDoc, now: u64) -> std::io::Result<()> {
        let high_water = if now > doc.issued_at + CLOCK_SKEW_SECS { doc.issued_at } else { now.max(doc.issued_at) };
        let c = CacheFile { envelope: Some(env.clone()), high_water };
        util::write_json_atomic(&self.file, &c)
    }

    pub fn clear(&self) {
        // The high-water mark stays: signing out and in again must not reset the clock check.
        let c = CacheFile { envelope: None, high_water: self.read().high_water };
        let _ = util::write_json_atomic(&self.file, &c);
    }

    /// The cached list, verified again, as of `now`. Moves the high-water mark forward (saved when it moved by more
    /// than a minute, so the 15-second list refresh doesn't write every time).
    pub fn snapshot(&self, keys: &Keys, account_id: &str, device_id: &str, now: u64) -> Snapshot {
        let mut c = self.read();
        let doc = c.envelope.as_ref().and_then(|e| parse_doc(keys, e, account_id, device_id).ok());
        let snap = Snapshot::evaluate(doc.as_ref(), now, c.high_water);
        if snap.state != ListState::ClockChanged && now > c.high_water + 60 {
            c.high_water = now;
            let _ = util::write_json_atomic(&self.file, &c);
        }
        snap
    }

    /// When the cached list was issued (to decide whether to ask the server again).
    pub fn issued_at(&self) -> Option<u64> {
        let env = self.read().envelope?;
        let bytes = B64.decode(env.payload.trim_end_matches('=')).ok()?;
        serde_json::from_slice::<EntitlementDoc>(&bytes).ok().map(|d| d.issued_at)
    }
}

// ---------- download grants ----------

/// Permission to download one paid pack once, soon. The `url` is itself short-lived and checked by the server or the
/// storage (a presigned link); the signed `sha256` is what the downloaded zip must match.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadGrant {
    pub typ: String,
    pub grant_id: String,
    pub account_id: String,
    pub device_id: String,
    pub game_id: String,
    pub version: String,
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
    pub url: String,
    pub issued_at: u64,
    pub expires_at: u64,
}

pub fn parse_grant(keys: &Keys, env: &Envelope, account_id: &str, device_id: &str, game_id: &str, now: u64) -> Result<DownloadGrant, String> {
    let bytes = verify_envelope(keys, env)?;
    let g: DownloadGrant = serde_json::from_slice(&bytes).map_err(|e| format!("Bad download grant: {e}"))?;
    if g.typ != GRANT_TYP {
        return Err(format!("Not a download grant: {}", g.typ));
    }
    if g.account_id != account_id || g.device_id != device_id || g.game_id != game_id {
        return Err("Download grant is for another account, device or game".into());
    }
    if g.expires_at <= g.issued_at || g.expires_at - g.issued_at > MAX_GRANT_SECS {
        return Err("Download grant has a bad lifetime".into());
    }
    if now > g.expires_at + CLOCK_SKEW_SECS || now + CLOCK_SKEW_SECS < g.issued_at {
        return Err("Download grant expired (or the clock is wrong)".into());
    }
    if !super::manifest::is_sha256(&g.sha256) {
        return Err("Download grant has no valid sha256".into());
    }
    if !g.url.starts_with("https://") && !util::dev_local_http(&g.url) {
        return Err("Download grant link must be https".into());
    }
    Ok(g)
}

/// Count of active entitlements per game (for logs and tests).
pub fn active_games(items: &[Entitlement]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for e in items.iter().filter(|e| e.status == EntStatus::Active) {
        *m.entry(e.game_id.clone()).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
pub mod test_keys {
    //! A throwaway signing key for tests (never used outside them).
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    pub fn signing(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    pub fn keys(seed: u8, kid: &str) -> Keys {
        Keys::parse(&format!("{kid}:{}", B64.encode(signing(seed).verifying_key().to_bytes()))).unwrap()
    }

    pub fn sign(seed: u8, kid: &str, payload: &serde_json::Value) -> Envelope {
        let bytes = serde_json::to_vec(payload).unwrap();
        let sig = signing(seed).sign(&bytes);
        Envelope { kid: kid.into(), payload: B64.encode(&bytes), sig: B64.encode(sig.to_bytes()) }
    }
}

#[cfg(test)]
mod tests {
    use super::test_keys::*;
    use super::*;
    use serde_json::json;

    const NOW: u64 = 1_800_000_000;

    fn doc(items: serde_json::Value) -> serde_json::Value {
        json!({ "typ": ENTITLEMENTS_TYP, "accountId": "acc_1", "deviceId": "dev_1", "issuedAt": NOW, "expiresAt": NOW + 14 * 86_400, "entitlements": items })
    }

    fn owned(game: &str) -> serde_json::Value {
        json!([{ "gameId": game, "source": "purchase", "status": "active", "grantedAt": "2027-01-15T00:00:00Z", "orderRef": "pi_1" }])
    }

    #[test]
    fn signature_verifies_and_tamper_fails() {
        let keys = keys(7, "k1");
        let env = sign(7, "k1", &doc(owned("paid-game")));
        let d = parse_doc(&keys, &env, "acc_1", "dev_1").unwrap();
        assert_eq!(d.entitlements[0].source, Source::Purchase);

        // payload changed after signing
        let mut bad = env.clone();
        let forged = serde_json::to_vec(&doc(owned("other-game"))).unwrap();
        bad.payload = B64.encode(forged);
        assert!(parse_doc(&keys, &bad, "acc_1", "dev_1").is_err());
        // signed by someone else's key under our kid
        assert!(parse_doc(&keys, &sign(8, "k1", &doc(owned("paid-game"))), "acc_1", "dev_1").is_err());
        // unknown kid
        assert!(parse_doc(&keys, &sign(7, "k2", &doc(owned("paid-game"))), "acc_1", "dev_1").is_err());
        // another account's or device's list
        assert!(parse_doc(&keys, &env, "acc_2", "dev_1").is_err());
        assert!(parse_doc(&keys, &env, "acc_1", "dev_2").is_err());
        // a download grant is not an entitlement list
        let mut g = doc(owned("paid-game"));
        g["typ"] = json!(GRANT_TYP);
        assert!(parse_doc(&keys, &sign(7, "k1", &g), "acc_1", "dev_1").is_err());
        // no keys built in
        assert!(parse_doc(&Keys::default(), &env, "acc_1", "dev_1").is_err());
        // key rotation: either key is accepted
        let both = Keys::parse(&format!("k0:{},k1:{}", B64.encode(signing(3).verifying_key().to_bytes()), B64.encode(signing(7).verifying_key().to_bytes()))).unwrap();
        assert!(parse_doc(&both, &env, "acc_1", "dev_1").is_ok());
        assert!(Keys::parse("k1:notbase64!").is_err());
        assert!(Keys::parse("").unwrap().is_empty());
    }

    /// Signed by the reference server (server/src/crypto.js) with the same test seed: both sides agree on the format.
    #[test]
    fn verifies_what_the_server_signs() {
        let env: Envelope = serde_json::from_str(r#"{"kid":"k1","payload":"eyJ0eXAiOiJ0aXJndW1pbS5lbnRpdGxlbWVudHMudjEiLCJhY2NvdW50SWQiOiJhY2NfMSIsImRldmljZUlkIjoiZGV2XzEiLCJpc3N1ZWRBdCI6MTgwMDAwMDAwMCwiZXhwaXJlc0F0IjoxODAxMjA5NjAwLCJlbnRpdGxlbWVudHMiOlt7ImdhbWVJZCI6InBhaWQtZ2FtZSIsInNvdXJjZSI6InB1cmNoYXNlIiwic3RhdHVzIjoiYWN0aXZlIiwiZ3JhbnRlZEF0IjoiMjAyNy0wMS0xNVQwODowMDowMFoiLCJvcmRlclJlZiI6Im9yZF8xIn1dfQ","sig":"tiT54Uf0i-NAnUolcrR4xJflnb02mGImhho7rIT_Kn9LzHbKc76LJ3CZS7RRWn60loWvXvD2eNppjU-8goEFBw"}"#).unwrap();
        let server_key = Keys::parse("k1:6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw").unwrap();
        let d = parse_doc(&server_key, &env, "acc_1", "dev_1").unwrap();
        assert!(Snapshot::evaluate(Some(&d), NOW + 60, NOW).owns("paid-game"));
        // and the test key here is the same key
        assert!(parse_doc(&keys(7, "k1"), &env, "acc_1", "dev_1").is_ok());
    }

    #[test]
    fn expiry_and_clock() {
        let d: EntitlementDoc = serde_json::from_value(doc(owned("paid-game"))).unwrap();
        assert!(Snapshot::evaluate(Some(&d), NOW + 3600, NOW).owns("paid-game"));
        // 14 days offline: expired
        let s = Snapshot::evaluate(Some(&d), NOW + 14 * 86_400, NOW);
        assert_eq!(s.state, ListState::Expired);
        assert!(!s.owns("paid-game"));
        // clock turned back below what was seen
        let s = Snapshot::evaluate(Some(&d), NOW + 3600, NOW + 20 * 86_400);
        assert_eq!(s.state, ListState::ClockChanged);
        // clock turned back below the list's own issue time
        assert_eq!(Snapshot::evaluate(Some(&d), NOW - 86_400, 0).state, ListState::ClockChanged);
        // a small difference between clocks is fine
        assert!(Snapshot::evaluate(Some(&d), NOW - 60, NOW).owns("paid-game"));
        // the launcher's own cap, even if the server signs a year
        let mut long = d.clone();
        long.expires_at = NOW + 365 * 86_400;
        assert_eq!(Snapshot::evaluate(Some(&long), NOW + 30 * 86_400, NOW).state, ListState::Expired);
    }

    #[test]
    fn gating() {
        let mut items = owned("paid-game");
        items.as_array_mut().unwrap().push(json!({ "gameId": "refunded-game", "source": "purchase", "status": "refunded", "grantedAt": "2027-01-01T00:00:00Z", "orderRef": "pi_2" }));
        items.as_array_mut().unwrap().push(json!({ "gameId": "revoked-game", "source": "admin-approved", "status": "revoked", "grantedAt": "2027-01-01T00:00:00Z" }));
        let d: EntitlementDoc = serde_json::from_value(doc(items)).unwrap();
        let s = Snapshot::evaluate(Some(&d), NOW, NOW);
        assert!(s.check(true, "paid-game").is_ok());
        assert_eq!(s.check(true, "refunded-game").unwrap_err().code, "NOT_ENTITLED");
        assert_eq!(s.access(true, "refunded-game"), "refunded");
        assert_eq!(s.access(true, "revoked-game"), "revoked");
        assert_eq!(s.check(true, "unknown").unwrap_err().code, "NOT_ENTITLED");
        // free games pass whatever the account state
        for st in [ListState::NotConfigured, ListState::SignedOut, ListState::None, ListState::Expired, ListState::ClockChanged] {
            assert!(Snapshot::plain(st).check(false, "free-game").is_ok());
            assert_eq!(Snapshot::plain(st).access(false, "free-game"), "free");
            assert!(Snapshot::plain(st).check(true, "paid-game").is_err());
        }
        assert_eq!(Snapshot::plain(ListState::SignedOut).check(true, "x").unwrap_err().code, "SIGN_IN_REQUIRED");
        assert_eq!(Snapshot::plain(ListState::NotConfigured).check(true, "x").unwrap_err().code, "ACCOUNTS_NOT_CONFIGURED");
        assert_eq!(Snapshot::evaluate(Some(&d), NOW + 15 * 86_400, NOW).check(true, "paid-game").unwrap_err().code, "ENTITLEMENTS_EXPIRED");
        assert_eq!(Snapshot::evaluate(Some(&d), NOW, NOW + 86_400).check(true, "paid-game").unwrap_err().code, "CLOCK_CHANGED");
        assert_eq!(active_games(&d.entitlements).len(), 1);
    }

    #[test]
    fn cache_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let keys = keys(7, "k1");
        let cache = Cache::new(dir.path());
        assert_eq!(cache.snapshot(&keys, "acc_1", "dev_1", NOW).state, ListState::None);
        let env = sign(7, "k1", &doc(owned("paid-game")));
        let d = parse_doc(&keys, &env, "acc_1", "dev_1").unwrap();
        cache.store(&env, &d, NOW).unwrap();
        assert!(cache.snapshot(&keys, "acc_1", "dev_1", NOW + 10).owns("paid-game"));
        assert_eq!(cache.issued_at(), Some(NOW));
        // the launcher ran at NOW + 2 days; the clock is then put back a day
        assert!(cache.snapshot(&keys, "acc_1", "dev_1", NOW + 2 * 86_400).owns("paid-game"));
        assert_eq!(cache.snapshot(&keys, "acc_1", "dev_1", NOW + 86_400).state, ListState::ClockChanged);
        // the file edited by hand: no longer valid
        let f = dir.path().join("account").join("entitlements.json");
        let text = std::fs::read_to_string(&f).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["envelope"]["payload"] = json!(B64.encode(serde_json::to_vec(&doc(owned("other-game"))).unwrap()));
        std::fs::write(&f, v.to_string()).unwrap();
        let s = cache.snapshot(&keys, "acc_1", "dev_1", NOW + 3 * 86_400);
        assert!(!s.owns("other-game") && !s.owns("paid-game"));
        // signing out and in again doesn't reset the clock check
        cache.clear();
        assert_eq!(cache.snapshot(&keys, "acc_1", "dev_1", NOW + 86_400).state, ListState::None);
        assert!(std::fs::read_to_string(&f).unwrap().contains(&(NOW + 3 * 86_400).to_string()));

        // the clock was once wrongly far ahead: a fresh list (server time) puts things right
        let later = |t: u64| sign(7, "k1", &json!({ "typ": ENTITLEMENTS_TYP, "accountId": "acc_1", "deviceId": "dev_1", "issuedAt": t, "expiresAt": t + 14 * 86_400, "entitlements": owned("paid-game") }));
        cache.snapshot(&keys, "acc_1", "dev_1", NOW + 400 * 86_400);
        let env3 = later(NOW + 3 * 86_400);
        cache.store(&env3, &parse_doc(&keys, &env3, "acc_1", "dev_1").unwrap(), NOW + 400 * 86_400).unwrap();
        assert!(cache.snapshot(&keys, "acc_1", "dev_1", NOW + 3 * 86_400 + 10).owns("paid-game"));
        // the clock turned back while online: the server's time shows it
        let env4 = later(NOW + 10 * 86_400);
        cache.store(&env4, &parse_doc(&keys, &env4, "acc_1", "dev_1").unwrap(), NOW + 5 * 86_400).unwrap();
        assert_eq!(cache.snapshot(&keys, "acc_1", "dev_1", NOW + 5 * 86_400).state, ListState::ClockChanged);
    }

    #[test]
    fn download_grants() {
        let keys = keys(7, "k1");
        let g = json!({ "typ": GRANT_TYP, "grantId": "g1", "accountId": "acc_1", "deviceId": "dev_1", "gameId": "paid-game",
            "version": "1.0.0", "sha256": "a".repeat(64), "size": 10, "url": "https://files.example/x.zip", "issuedAt": NOW, "expiresAt": NOW + 600 });
        let env = sign(7, "k1", &g);
        assert_eq!(parse_grant(&keys, &env, "acc_1", "dev_1", "paid-game", NOW + 10).unwrap().version, "1.0.0");
        assert!(parse_grant(&keys, &env, "acc_1", "dev_1", "other-game", NOW).is_err());
        assert!(parse_grant(&keys, &env, "acc_2", "dev_1", "paid-game", NOW).is_err());
        assert!(parse_grant(&keys, &env, "acc_1", "dev_1", "paid-game", NOW + 2000).is_err());
        let mut long = g.clone();
        long["expiresAt"] = json!(NOW + 86_400);
        assert!(parse_grant(&keys, &sign(7, "k1", &long), "acc_1", "dev_1", "paid-game", NOW).is_err());
        let mut http = g.clone();
        http["url"] = json!("http://files.example/x.zip");
        assert!(parse_grant(&keys, &sign(7, "k1", &http), "acc_1", "dev_1", "paid-game", NOW).is_err());
        let mut list = g.clone();
        list["typ"] = json!(ENTITLEMENTS_TYP);
        assert!(parse_grant(&keys, &sign(7, "k1", &list), "acc_1", "dev_1", "paid-game", NOW).is_err());
    }
}
