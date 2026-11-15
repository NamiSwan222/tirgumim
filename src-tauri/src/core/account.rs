//! The player's account on the Tirgumim account server: email sign-in (a link in an email, confirmed in the browser,
//! no password), the session token kept in the Windows credential store, and the calls for entitlements, the store,
//! checkout and download grants. The server contract is in ENTITLEMENTS.md.
//!
//! Without a server address and public keys built in (`ACCOUNT_URL`, `ENTITLEMENT_KEYS`) everything here answers
//! "not configured" and free games work exactly as before.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::catalog::{ResolvedPack, USER_AGENT};
use super::entitlements::{self, Cache, DownloadGrant, Envelope, Keys, ListState, Snapshot};
use super::installer::PackError;
use super::util;

/// The account server, e.g. "https://accounts.example.org". Set at build time with the `TIRGUMIM_ACCOUNT_URL`
/// environment variable (or write it here once the server exists). Empty: accounts are off.
pub const ACCOUNT_URL: &str = match option_env!("TIRGUMIM_ACCOUNT_URL") {
    Some(u) => u,
    None => "",
};

/// The server's Ed25519 public keys, `kid:base64url` separated by commas (see `server/scripts/keygen.mjs`). Public
/// keys are not secret: once generated, write them here. Build-time `TIRGUMIM_ENTITLEMENT_KEYS` overrides.
pub const ENTITLEMENT_KEYS: &str = match option_env!("TIRGUMIM_ENTITLEMENT_KEYS") {
    Some(k) => k,
    None => "",
};

/// How long a sign-in link waits to be confirmed (the server decides too).
pub const SIGN_IN_WAIT_SECS: u64 = 15 * 60;
/// A list older than this is fetched again when the launcher next asks.
pub const REFRESH_AFTER_SECS: u64 = 3600;

fn err(code: &str, msg: impl Into<String>) -> PackError {
    PackError::new(code, msg)
}

// ---------- configuration ----------

#[derive(Clone, Default)]
pub struct Config {
    pub base_url: Option<String>,
    pub keys: Keys,
}

impl Config {
    /// What this build was made with. Debug builds may also take `TIRGUMIM_ACCOUNT_URL` /
    /// `TIRGUMIM_ENTITLEMENT_KEYS` at run time (for the reference server on this PC); release builds never do, so
    /// a player can't point the launcher at a server and key of their own.
    pub fn from_build() -> Config {
        let mut url = ACCOUNT_URL.to_string();
        let mut keys = ENTITLEMENT_KEYS.to_string();
        if cfg!(debug_assertions) {
            if let Ok(u) = std::env::var("TIRGUMIM_ACCOUNT_URL") {
                url = u;
            }
            if let Ok(k) = std::env::var("TIRGUMIM_ENTITLEMENT_KEYS") {
                keys = k;
            }
        }
        Config::new(&url, &keys)
    }

    pub fn new(url: &str, keys: &str) -> Config {
        let url = url.trim().trim_end_matches('/');
        let ok_url = url.starts_with("https://") || util::dev_local_http(url);
        let keys = Keys::parse(keys).unwrap_or_else(|e| {
            eprintln!("account: bad entitlement keys: {e}");
            Keys::default()
        });
        if !url.is_empty() && !ok_url {
            eprintln!("account: server address must be https: {url}");
        }
        Config { base_url: (ok_url && !url.is_empty()).then(|| url.to_string()), keys }
    }

    pub fn configured(&self) -> bool {
        self.base_url.is_some() && !self.keys.is_empty()
    }
}

// ---------- where the token is kept ----------

/// Keeps one secret (the session). The real one is the OS credential store; tests use memory.
pub trait SecretStore: Send + Sync {
    fn get(&self) -> Result<Option<String>, String>;
    fn set(&self, value: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

/// Windows Credential Manager (Keychain on macOS, the kernel keyring on Linux), encrypted for the Windows user.
pub struct KeyringStore {
    service: String,
    user: String,
}

impl KeyringStore {
    pub fn new(service: &str, user: &str) -> KeyringStore {
        KeyringStore { service: service.into(), user: user.into() }
    }
    fn entry(&self) -> Result<keyring::Entry, String> {
        keyring::Entry::new(&self.service, &self.user).map_err(|e| e.to_string())
    }
}

impl SecretStore for KeyringStore {
    fn get(&self) -> Result<Option<String>, String> {
        match self.entry()?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn set(&self, value: &str) -> Result<(), String> {
        self.entry()?.set_password(value).map_err(|e| e.to_string())
    }
    fn delete(&self) -> Result<(), String> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[derive(Default)]
pub struct MemoryStore(Mutex<Option<String>>);

impl SecretStore for MemoryStore {
    fn get(&self) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn set(&self, value: &str) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(value.into());
        Ok(())
    }
    fn delete(&self) -> Result<(), String> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

// ---------- talking to the server ----------

/// One JSON request. Returns the HTTP status and the JSON body (`null` when there is none).
pub trait Http: Send + Sync {
    fn call(&self, method: &str, url: &str, bearer: Option<&str>, body: Option<&Value>) -> Result<(u16, Value), String>;
}

pub struct UreqHttp(ureq::Agent);

impl Default for UreqHttp {
    fn default() -> Self {
        UreqHttp(
            ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_recv_response(Some(Duration::from_secs(20)))
                .user_agent(USER_AGENT)
                .http_status_as_error(false)
                .build()
                .into(),
        )
    }
}

impl Http for UreqHttp {
    fn call(&self, method: &str, url: &str, bearer: Option<&str>, body: Option<&Value>) -> Result<(u16, Value), String> {
        let auth = bearer.map(|t| format!("Bearer {t}"));
        let mut resp = match method {
            "GET" => {
                let mut r = self.0.get(url).header("Accept", "application/json");
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                r.call()
            }
            _ => {
                let mut r = self.0.post(url).header("Accept", "application/json").header("Content-Type", "application/json");
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                r.send(body.cloned().unwrap_or(json!({})).to_string())
            }
        }
        .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().with_config().limit(2 * 1024 * 1024).read_to_string().unwrap_or_default();
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }
}

// ---------- state ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    token: String,
    account_id: String,
    email: String,
}

#[derive(Debug, Clone)]
struct Pending {
    request_id: String,
    poll_secret: String,
    email: String,
    code: String,
    expires_at: u64,
}

/// What the window shows about the account.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountState {
    /// The launcher has an account server and keys.
    pub configured: bool,
    /// "notConfigured" | "signedOut" | "pending" | "signedIn"
    pub status: &'static str,
    pub email: Option<String>,
    pub account_id: Option<String>,
    /// While waiting for the email link: the code the browser page shows too, and when the wait ends (unix secs).
    pub pending: Option<Value>,
}

/// What a poll of a pending sign-in found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poll {
    Nothing,
    Waiting,
    SignedIn,
    Expired,
}

pub struct Accounts {
    cfg: Config,
    store: Box<dyn SecretStore>,
    http: Box<dyn Http>,
    cache: Cache,
    device_file: PathBuf,
    session: Mutex<Option<Option<Session>>>,
    pending: Mutex<Option<Pending>>,
    last_refresh_try: Mutex<u64>,
}

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("no randomness from the OS");
    util::hex(&b)
}

impl Accounts {
    pub fn new(data_dir: &Path, cfg: Config, store: Box<dyn SecretStore>, http: Box<dyn Http>) -> Accounts {
        Accounts {
            cfg,
            store,
            http,
            cache: Cache::new(data_dir),
            device_file: data_dir.join("account").join("device.json"),
            session: Mutex::new(None),
            pending: Mutex::new(None),
            last_refresh_try: Mutex::new(0),
        }
    }

    pub fn configured(&self) -> bool {
        self.cfg.configured()
    }

    /// A random id for this PC's launcher, made once. Signed lists and grants name it, so a copied list doesn't work
    /// on another PC. Not a secret.
    pub fn device_id(&self) -> String {
        #[derive(Serialize, Deserialize)]
        struct D {
            id: String,
        }
        if let Some(d) = util::read_json::<D>(&self.device_file).filter(|d| d.id.len() >= 16) {
            return d.id;
        }
        let id = format!("dev_{}", random_hex(16));
        let _ = util::write_json_atomic(&self.device_file, &D { id: id.clone() });
        id
    }

    fn session(&self) -> Option<Session> {
        let mut s = self.session.lock().unwrap();
        if s.is_none() {
            let loaded = self.store.get().ok().flatten().and_then(|t| serde_json::from_str::<Session>(&t).ok());
            *s = Some(loaded);
        }
        s.clone().flatten()
    }

    fn set_session(&self, v: Option<Session>) -> Result<(), PackError> {
        match &v {
            Some(s) => self.store.set(&serde_json::to_string(s).unwrap()).map_err(|e| err("SECURE_STORE", format!("Could not save the sign-in: {e}")))?,
            None => {
                let _ = self.store.delete();
            }
        }
        *self.session.lock().unwrap() = Some(v);
        Ok(())
    }

    pub fn state(&self) -> AccountState {
        let mut a = AccountState { configured: self.configured(), status: "notConfigured", email: None, account_id: None, pending: None };
        if !a.configured {
            return a;
        }
        if let Some(s) = self.session() {
            a.status = "signedIn";
            a.email = Some(s.email);
            a.account_id = Some(s.account_id);
        } else if let Some(p) = self.pending.lock().unwrap().as_ref() {
            a.status = "pending";
            a.email = Some(p.email.clone());
            a.pending = Some(json!({ "email": p.email, "code": p.code, "expiresAt": p.expires_at }));
        } else {
            a.status = "signedOut";
        }
        a
    }

    fn url(&self, path: &str) -> Result<String, PackError> {
        let base = self.cfg.base_url.as_deref().filter(|_| self.configured()).ok_or_else(|| err("ACCOUNTS_NOT_CONFIGURED", "No account server in this launcher"))?;
        Ok(format!("{base}{path}"))
    }

    /// A server call. Turns the server's `{"error":{"code","message"}}` into a PackError with that code; a 401 on
    /// a signed-in call signs out (the session was revoked or expired).
    fn call(&self, method: &str, path: &str, auth: bool, body: Option<&Value>) -> Result<Value, PackError> {
        let url = self.url(path)?;
        let session = if auth { Some(self.session().ok_or_else(|| err("SIGN_IN_REQUIRED", "Not signed in"))?) } else { None };
        let (status, v) = self.http.call(method, &url, session.as_ref().map(|s| s.token.as_str()), body).map_err(|e| err("ACCOUNT_SERVER", e))?;
        if (200..300).contains(&status) {
            return Ok(v);
        }
        let code = v.pointer("/error/code").and_then(Value::as_str).map(str::to_string);
        let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("").to_string();
        if status == 401 && auth {
            let _ = self.set_session(None);
            self.cache.clear();
            return Err(err("SIGN_IN_EXPIRED", "Signed out: sign in again"));
        }
        let code = match (status, code) {
            (_, Some(c)) if c.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_') && !c.is_empty() => c,
            (429, _) => "RATE_LIMITED".into(),
            _ => "ACCOUNT_SERVER".into(),
        };
        Err(err(&code, if msg.is_empty() { format!("Account server answered {status}") } else { msg }))
    }

    // ---------- sign-in ----------

    /// Asks the server to email a sign-in link. The launcher then polls (`poll_sign_in`) until the link is opened
    /// and confirmed in the browser. Only this launcher can collect the session: it alone knows the poll secret.
    pub fn start_sign_in(&self, email: &str) -> Result<AccountState, PackError> {
        let email = email.trim().to_lowercase();
        let valid = email.len() <= 254 && email.split_once('@').is_some_and(|(a, d)| !a.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.')) && !email.contains(char::is_whitespace);
        if !valid {
            return Err(err("BAD_EMAIL", "That is not an email address"));
        }
        let secret = random_hex(32);
        let v = self.call("POST", "/v1/auth/start", false, Some(&json!({
            "email": email, "deviceId": self.device_id(), "deviceName": std::env::var("COMPUTERNAME").unwrap_or_default(),
            "pollHash": util::sha256_bytes(secret.as_bytes()),
        })))?;
        let request_id = v.get("requestId").and_then(Value::as_str).ok_or_else(|| err("ACCOUNT_SERVER", "No requestId"))?.to_string();
        let code = v.get("code").and_then(Value::as_str).unwrap_or("").to_string();
        let expires_at = v.get("expiresAt").and_then(Value::as_u64).unwrap_or(util::now_secs() + SIGN_IN_WAIT_SECS);
        *self.pending.lock().unwrap() = Some(Pending { request_id, poll_secret: secret, email, code, expires_at });
        Ok(self.state())
    }

    pub fn cancel_sign_in(&self) {
        *self.pending.lock().unwrap() = None;
    }

    pub fn poll_sign_in(&self) -> Result<Poll, PackError> {
        let Some(p) = self.pending.lock().unwrap().clone() else { return Ok(Poll::Nothing) };
        if util::now_secs() > p.expires_at + 60 {
            self.cancel_sign_in();
            return Ok(Poll::Expired);
        }
        let v = match self.call("POST", "/v1/auth/poll", false, Some(&json!({ "requestId": p.request_id, "pollSecret": p.poll_secret }))) {
            Ok(v) => v,
            Err(e) if e.code == "SIGN_IN_EXPIRED" || e.code == "AUTH_REQUEST_EXPIRED" || e.code == "AUTH_REQUEST_UNKNOWN" => {
                self.cancel_sign_in();
                return Ok(Poll::Expired);
            }
            Err(e) => return Err(e),
        };
        match v.get("status").and_then(Value::as_str) {
            Some("approved") => {
                let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
                let (Some(token), Some(account_id)) = (s("sessionToken"), s("accountId")) else { return Err(err("ACCOUNT_SERVER", "Sign-in answer is missing fields")) };
                let email = s("email").unwrap_or(p.email);
                self.cancel_sign_in();
                // Another account's list must never be used for this one.
                self.cache.clear();
                self.set_session(Some(Session { token, account_id, email }))?;
                let _ = self.refresh();
                Ok(Poll::SignedIn)
            }
            Some("expired") => {
                self.cancel_sign_in();
                Ok(Poll::Expired)
            }
            _ => Ok(Poll::Waiting),
        }
    }

    /// Signs out here and tells the server to end the session (best effort: offline, the token is still forgotten).
    pub fn sign_out(&self) -> AccountState {
        if self.session().is_some() {
            let _ = self.call("POST", "/v1/auth/signout", true, None);
        }
        let _ = self.set_session(None);
        self.cancel_sign_in();
        self.cache.clear();
        self.state()
    }

    // ---------- entitlements ----------

    /// Fetches and verifies a fresh signed list and caches it.
    pub fn refresh(&self) -> Result<Snapshot, PackError> {
        *self.last_refresh_try.lock().unwrap() = util::now_secs();
        let s = self.session().ok_or_else(|| err("SIGN_IN_REQUIRED", "Not signed in"))?;
        let v = self.call("GET", "/v1/entitlements", true, None)?;
        let env: Envelope = serde_json::from_value(v).map_err(|e| err("ENTITLEMENTS_INVALID", e.to_string()))?;
        let device = self.device_id();
        let doc = entitlements::parse_doc(&self.cfg.keys, &env, &s.account_id, &device).map_err(|e| err("ENTITLEMENTS_INVALID", e))?;
        let now = util::now_secs();
        self.cache.store(&env, &doc, now)?;
        Ok(self.snapshot())
    }

    /// Fetches again when the cached list is over an hour old (at most one try every 5 minutes, so being offline
    /// doesn't cost a request every list refresh). Errors are ignored: the cached list stays in use.
    pub fn refresh_if_stale(&self) {
        if !self.configured() || self.session().is_none() {
            return;
        }
        let now = util::now_secs();
        let stale = self.cache.issued_at().is_none_or(|t| now.saturating_sub(t) > REFRESH_AFTER_SECS);
        if stale && now.saturating_sub(*self.last_refresh_try.lock().unwrap()) > 300 {
            if let Err(e) = self.refresh() {
                eprintln!("entitlements: {e}");
            }
        }
    }

    /// What the launcher knows now, from the cached signed list.
    pub fn snapshot(&self) -> Snapshot {
        if !self.configured() {
            return Snapshot::plain(ListState::NotConfigured);
        }
        let Some(s) = self.session() else { return Snapshot::plain(ListState::SignedOut) };
        self.cache.snapshot(&self.cfg.keys, &s.account_id, &self.device_id(), util::now_secs())
    }

    pub fn owns(&self, game_id: &str) -> bool {
        self.snapshot().owns(game_id)
    }

    /// The gate: Ok for free games and for paid games this account owns.
    pub fn check_access(&self, paid: bool, game_id: &str) -> Result<(), PackError> {
        if !paid {
            return Ok(());
        }
        let snap = self.snapshot();
        if snap.state == ListState::None {
            // Signed in but nothing cached yet: one try now.
            return self.refresh()?.check(true, game_id);
        }
        snap.check(true, game_id)
    }

    // ---------- store, packs, checkout, downloads ----------

    /// The server's price list: `{ "paymentsConfigured": bool, "products": [{ gameId, priceId, amount, currency, active }] }`.
    pub fn store(&self) -> Result<Value, PackError> {
        self.call("GET", "/v1/store", false, None)
    }

    /// The newest paid pack's version and checksum (public facts; downloading it needs a grant).
    pub fn pack_info(&self, game_id: &str) -> Result<ResolvedPack, String> {
        let v = self.call("GET", &format!("/v1/packs/{}", enc(game_id)), false, None).map_err(|e| if e.code == "NO_PACK" { super::catalog::NO_RELEASE.to_string() } else { e.message })?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let (Some(version), Some(sha256)) = (s("version"), s("sha256")) else { return Err("Pack answer is missing fields".into()) };
        Ok(ResolvedPack {
            version,
            url: String::new(),
            sha256,
            size: v.get("size").and_then(Value::as_u64),
            changelog: s("changelog"),
            published_at: s("publishedAt"),
            source: "server".into(),
        })
    }

    /// Asks for a download grant for an owned paid pack and checks it.
    pub fn download_grant(&self, game_id: &str) -> Result<DownloadGrant, PackError> {
        let s = self.session().ok_or_else(|| err("SIGN_IN_REQUIRED", "Not signed in"))?;
        let v = self.call("POST", "/v1/download-grants", true, Some(&json!({ "gameId": game_id })))?;
        let env: Envelope = serde_json::from_value(v).map_err(|e| err("GRANT_INVALID", e.to_string()))?;
        entitlements::parse_grant(&self.cfg.keys, &env, &s.account_id, &self.device_id(), game_id, util::now_secs()).map_err(|e| err("GRANT_INVALID", e))
    }

    /// Starts a hosted checkout (Stripe Checkout or another provider's page) and returns its https address.
    pub fn start_checkout(&self, game_id: &str) -> Result<String, PackError> {
        if !self.configured() {
            return Err(err("PAYMENTS_NOT_CONFIGURED", "This launcher has no account server yet"));
        }
        if self.owns(game_id) {
            return Err(err("ALREADY_OWNED", "This translation is already in your account"));
        }
        let v = self.call("POST", "/v1/checkout", true, Some(&json!({ "gameId": game_id })))?;
        let url = v.get("url").and_then(Value::as_str).unwrap_or("");
        if !url.starts_with("https://") && !util::dev_local_http(url) {
            return Err(err("ACCOUNT_SERVER", "Checkout address is not https"));
        }
        Ok(url.to_string())
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entitlements::test_keys::{keys, sign};
    use crate::core::entitlements::{ENTITLEMENTS_TYP, GRANT_TYP};
    use std::collections::VecDeque;
    use std::sync::Arc;

    /// method, url, bearer token, body
    type Asked = (String, String, Option<String>, Option<Value>);

    /// A scripted server: answers in order, records what was asked.
    #[derive(Default, Clone)]
    struct Fake {
        answers: Arc<Mutex<VecDeque<(u16, Value)>>>,
        asked: Arc<Mutex<Vec<Asked>>>,
    }
    impl Fake {
        fn answer(&self, status: u16, v: Value) {
            self.answers.lock().unwrap().push_back((status, v));
        }
        fn last(&self) -> Asked {
            self.asked.lock().unwrap().last().cloned().unwrap()
        }
    }
    impl Http for Fake {
        fn call(&self, method: &str, url: &str, bearer: Option<&str>, body: Option<&Value>) -> Result<(u16, Value), String> {
            self.asked.lock().unwrap().push((method.into(), url.into(), bearer.map(str::to_string), body.cloned()));
            self.answers.lock().unwrap().pop_front().ok_or_else(|| "offline".to_string())
        }
    }

    fn setup(dir: &Path) -> (Accounts, Fake) {
        let fake = Fake::default();
        let cfg = Config { base_url: Some("https://accounts.example".into()), keys: keys(9, "k1") };
        (Accounts::new(dir, cfg, Box::new(MemoryStore::default()), Box::new(fake.clone())), fake)
    }

    fn list(a: &Accounts, items: Value) -> Value {
        let now = util::now_secs();
        let env = sign(9, "k1", &json!({ "typ": ENTITLEMENTS_TYP, "accountId": "acc_1", "deviceId": a.device_id(), "issuedAt": now, "expiresAt": now + 14 * 86_400, "entitlements": items }));
        serde_json::to_value(env).unwrap()
    }

    fn sign_in(a: &Accounts, fake: &Fake, items: Value) {
        fake.answer(200, json!({ "requestId": "req_1", "code": "4821", "expiresAt": util::now_secs() + 900 }));
        let st = a.start_sign_in(" Player@Example.COM ").unwrap();
        assert_eq!(st.status, "pending");
        assert_eq!(st.email.as_deref(), Some("player@example.com"));
        let (_, url, _, body) = fake.last();
        assert!(url.ends_with("/v1/auth/start"));
        let body = body.unwrap();
        assert_eq!(body["pollHash"].as_str().unwrap().len(), 64);
        fake.answer(200, json!({ "status": "pending" }));
        assert_eq!(a.poll_sign_in().unwrap(), Poll::Waiting);
        fake.answer(200, json!({ "status": "approved", "sessionToken": "tok_secret", "accountId": "acc_1", "email": "player@example.com" }));
        fake.answer(200, list(a, items));
        assert_eq!(a.poll_sign_in().unwrap(), Poll::SignedIn);
    }

    #[test]
    fn not_configured_keeps_free_games_working() {
        let dir = tempfile::tempdir().unwrap();
        let a = Accounts::new(dir.path(), Config::new("", ""), Box::new(MemoryStore::default()), Box::new(Fake::default()));
        assert_eq!(a.state().status, "notConfigured");
        assert!(a.check_access(false, "free-game").is_ok());
        assert_eq!(a.check_access(true, "paid-game").unwrap_err().code, "ACCOUNTS_NOT_CONFIGURED");
        assert_eq!(a.start_checkout("paid-game").unwrap_err().code, "PAYMENTS_NOT_CONFIGURED");
        assert_eq!(a.start_sign_in("a@b.co").unwrap_err().code, "ACCOUNTS_NOT_CONFIGURED");
        assert_eq!(a.snapshot().state, ListState::NotConfigured);
        // http (not https) server addresses are refused outside localhost
        assert!(Config::new("http://accounts.example", "").base_url.is_none());
        assert!(!Config::new("https://accounts.example", "").configured());
    }

    #[test]
    fn sign_in_entitlements_and_sign_out() {
        let dir = tempfile::tempdir().unwrap();
        let (a, fake) = setup(dir.path());
        assert_eq!(a.state().status, "signedOut");
        assert_eq!(a.check_access(true, "paid-game").unwrap_err().code, "SIGN_IN_REQUIRED");
        assert_eq!(a.start_sign_in("not-an-email").unwrap_err().code, "BAD_EMAIL");
        sign_in(&a, &fake, json!([{ "gameId": "paid-game", "source": "admin-approved", "status": "active", "grantedAt": "2027-01-01T00:00:00Z" }]));
        let st = a.state();
        assert_eq!((st.status, st.account_id.as_deref()), ("signedIn", Some("acc_1")));
        // the list was fetched with the session token
        assert_eq!(fake.last().2.as_deref(), Some("tok_secret"));
        assert!(a.owns("paid-game"));
        assert!(a.check_access(true, "paid-game").is_ok());
        assert_eq!(a.check_access(true, "other-paid").unwrap_err().code, "NOT_ENTITLED");
        // offline: the cached list still works
        assert!(a.check_access(true, "paid-game").is_ok());

        // a list signed with another key is refused and the old one stays
        let now = util::now_secs();
        let forged = sign(1, "k1", &json!({ "typ": ENTITLEMENTS_TYP, "accountId": "acc_1", "deviceId": a.device_id(), "issuedAt": now, "expiresAt": now + 86_400, "entitlements": [{ "gameId": "other-paid", "source": "purchase", "status": "active", "grantedAt": "x" }] }));
        fake.answer(200, serde_json::to_value(forged).unwrap());
        assert_eq!(a.refresh().unwrap_err().code, "ENTITLEMENTS_INVALID");
        assert!(!a.owns("other-paid") && a.owns("paid-game"));

        // revoked on the server: the next list says so
        fake.answer(200, list(&a, json!([{ "gameId": "paid-game", "source": "admin-approved", "status": "revoked", "grantedAt": "2027-01-01T00:00:00Z" }])));
        a.refresh().unwrap();
        assert_eq!(a.check_access(true, "paid-game").unwrap_err().code, "NOT_ENTITLED");

        // sign out forgets the token and the list
        fake.answer(200, json!({ "ok": true }));
        assert_eq!(a.sign_out().status, "signedOut");
        assert_eq!(a.check_access(true, "paid-game").unwrap_err().code, "SIGN_IN_REQUIRED");
    }

    #[test]
    fn server_ends_session() {
        let dir = tempfile::tempdir().unwrap();
        let (a, fake) = setup(dir.path());
        sign_in(&a, &fake, json!([]));
        fake.answer(401, json!({ "error": { "code": "UNAUTHORIZED", "message": "session revoked" } }));
        assert_eq!(a.refresh().unwrap_err().code, "SIGN_IN_EXPIRED");
        assert_eq!(a.state().status, "signedOut");
    }

    #[test]
    fn checkout_and_grants() {
        let dir = tempfile::tempdir().unwrap();
        let (a, fake) = setup(dir.path());
        sign_in(&a, &fake, json!([{ "gameId": "owned-game", "source": "purchase", "status": "active", "grantedAt": "2027-01-01T00:00:00Z", "orderRef": "pi_1" }]));
        assert_eq!(a.start_checkout("owned-game").unwrap_err().code, "ALREADY_OWNED");
        fake.answer(503, json!({ "error": { "code": "PAYMENTS_NOT_CONFIGURED", "message": "no provider" } }));
        assert_eq!(a.start_checkout("paid-game").unwrap_err().code, "PAYMENTS_NOT_CONFIGURED");
        fake.answer(200, json!({ "url": "https://checkout.stripe.com/c/pay/cs_test_1" }));
        assert_eq!(a.start_checkout("paid-game").unwrap(), "https://checkout.stripe.com/c/pay/cs_test_1");
        fake.answer(200, json!({ "url": "javascript:alert(1)" }));
        assert!(a.start_checkout("paid-game").is_err());

        let now = util::now_secs();
        let grant = |game: &str| {
            serde_json::to_value(sign(9, "k1", &json!({ "typ": GRANT_TYP, "grantId": "g1", "accountId": "acc_1", "deviceId": a.device_id(), "gameId": game,
                "version": "1.0.0", "sha256": "c".repeat(64), "size": 5, "url": "https://accounts.example/v1/files/g1", "issuedAt": now, "expiresAt": now + 600 }))).unwrap()
        };
        fake.answer(200, grant("owned-game"));
        assert_eq!(a.download_grant("owned-game").unwrap().sha256, "c".repeat(64));
        // a grant for another game is refused
        fake.answer(200, grant("other-game"));
        assert_eq!(a.download_grant("owned-game").unwrap_err().code, "GRANT_INVALID");
        fake.answer(403, json!({ "error": { "code": "NOT_ENTITLED", "message": "no" } }));
        assert_eq!(a.download_grant("owned-game").unwrap_err().code, "NOT_ENTITLED");
    }
}
