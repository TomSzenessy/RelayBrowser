//! Pure, platform-independent login model: origin binding, credential
//! candidate matching, explicit-fill gating, the save/decline state machine,
//! and secret redaction.
//!
//! Everything here is I/O-free: no WebKit, no Keychain, no SQLite, and no
//! bridge. The macOS integration must keep passwords out of the injected
//! bridge, the DOM snapshot, the MCP server, and logs; this module supplies the
//! decisions and the secret wrapper those paths share. The design and the open
//! owner decisions are in `docs/login-autofill.md` (issue #57).

use std::collections::HashSet;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

/// Maximum number of credentials offered for one login form.
pub const MAX_CANDIDATES: usize = 5;
/// Longest username accepted from a Keychain item or a captured form.
pub const MAX_USERNAME_LEN: usize = 256;
/// Longest password the native path will hold.
pub const MAX_PASSWORD_LEN: usize = 4096;
/// Longest page path used for candidate ranking.
pub const MAX_PATH_LEN: usize = 1024;

/// Why a URL is not an allowed credential origin.
///
/// Variants deliberately carry no URL text: an error string must never echo a
/// URL that may contain embedded credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OriginError {
    #[error("empty URL")]
    Empty,
    #[error("not an absolute http(s) URL")]
    NotAbsolute,
    #[error("plain http is refused except on loopback")]
    Insecure,
    #[error("URLs with embedded credentials are refused")]
    UserInfo,
    #[error("invalid host")]
    Host,
    #[error("invalid port")]
    Port,
    #[error("invalid path")]
    Path,
}

/// A credential origin: scheme plus canonical host plus a non-default port.
///
/// Binding is exact. There is no eTLD+1, `www.`-stripping, or suffix alias: a
/// login saved for `https://example.com` is not offered on
/// `https://www.example.com` until an owner-approved rule says otherwise.
/// Plain `http` is accepted only for `localhost` and loopback literals, as the
/// issue requires; everything else must be `https`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    https: bool,
    host: String,
    port: Option<u16>,
}

impl Origin {
    /// Parse an absolute URL and keep only its credential origin. Query and
    /// fragment are ignored; the path is checked but not retained.
    pub fn parse(url: &str) -> Result<Self, OriginError> {
        Self::parse_with_path(url).map(|(origin, _)| origin)
    }

    /// Parse an absolute URL into its origin and normalized path. The path is
    /// only a ranking hint; it never widens the origin binding.
    pub fn parse_with_path(url: &str) -> Result<(Self, String), OriginError> {
        if url.is_empty() {
            return Err(OriginError::Empty);
        }
        // Browsers percent-encode these, so a raw whitespace, control, or
        // backslash is malformed input. Rejecting early also keeps userinfo
        // from being mistaken for a host.
        if url.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\') {
            return Err(OriginError::NotAbsolute);
        }
        let (scheme, rest) = url.split_once("://").ok_or(OriginError::NotAbsolute)?;
        if scheme.is_empty()
            || !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        {
            return Err(OriginError::NotAbsolute);
        }
        let https = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            _ => return Err(OriginError::NotAbsolute),
        };
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        if authority.is_empty() {
            return Err(OriginError::Host);
        }
        if authority.contains('@') {
            return Err(OriginError::UserInfo);
        }
        // A percent-encoded authority is not a canonical host (this also
        // refuses IPv6 zone ids, which are not origin-stable).
        if authority.contains('%') {
            return Err(OriginError::Host);
        }
        let (host, port_text) = split_authority(authority)?;
        let host = normalize_host(&host)?;
        if !https && !is_loopback_host(&host) {
            return Err(OriginError::Insecure);
        }
        let port = parse_port(port_text.as_deref(), https)?;
        let path = normalize_path(&rest[authority_end..]).ok_or(OriginError::Path)?;
        Ok((Self { https, host, port }, path))
    }

    pub fn is_https(&self) -> bool {
        self.https
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// `None` means the scheme's default port (443 or 80).
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or(if self.https { 443 } else { 80 })
    }

    pub fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }

    /// `true` for every origin this model accepts: HTTPS, or loopback HTTP.
    pub fn is_secure(&self) -> bool {
        self.https || self.is_loopback()
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.https { "https://" } else { "http://" })?;
        if self.host.contains(':') {
            write!(f, "[{}]", self.host)?;
        } else {
            f.write_str(&self.host)?;
        }
        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }
        Ok(())
    }
}

fn split_authority(authority: &str) -> Result<(String, Option<String>), OriginError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or(OriginError::Host)?;
        let inside = &rest[..end];
        let after = &rest[end + 1..];
        let port = if after.is_empty() {
            None
        } else {
            Some(after.strip_prefix(':').ok_or(OriginError::Host)?.to_string())
        };
        return Ok((format!("[{inside}]"), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.contains(':') {
                return Err(OriginError::Host);
            }
            Ok((host.to_string(), Some(port.to_string())))
        }
        None => Ok((authority.to_string(), None)),
    }
}

fn normalize_host(host: &str) -> Result<String, OriginError> {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        let ip: Ipv6Addr = inner.parse().map_err(|_| OriginError::Host)?;
        return Ok(ip.to_string());
    }
    if host.is_empty() || host.len() > 253 {
        return Err(OriginError::Host);
    }
    let host = host.to_ascii_lowercase();
    if host.bytes().any(|b| !(b.is_ascii_alphanumeric() || b == b'-' || b == b'.')) {
        return Err(OriginError::Host);
    }
    if host.starts_with('.') || host.ends_with('.') || host.contains("..") {
        return Err(OriginError::Host);
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-') {
            return Err(OriginError::Host);
        }
    }
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return host.parse::<Ipv4Addr>().map(|ip| ip.to_string()).map_err(|_| OriginError::Host);
    }
    Ok(host)
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
        || host.parse::<Ipv6Addr>().is_ok_and(|ip| ip.is_loopback())
}

fn parse_port(text: Option<&str>, https: bool) -> Result<Option<u16>, OriginError> {
    let Some(text) = text else {
        return Ok(None);
    };
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(OriginError::Port);
    }
    let port: u16 = text.parse().map_err(|_| OriginError::Port)?;
    if port == 0 {
        return Err(OriginError::Port);
    }
    let default = if https { 443 } else { 80 };
    Ok((port != default).then_some(port))
}

/// Normalize a path for ranking. Empty paths become `/`; trailing slashes are
/// removed; query, fragment, control characters, backslashes, and `..`
/// segments are refused. The page path is never part of the origin identity.
fn normalize_path(raw: &str) -> Option<String> {
    let raw = raw.split(['?', '#']).next().unwrap_or("");
    if raw.is_empty() {
        return Some("/".to_string());
    }
    if raw.len() > MAX_PATH_LEN || !raw.starts_with('/') {
        return None;
    }
    if raw.chars().any(|c| c.is_control() || c == '\\') {
        return None;
    }
    if raw.split('/').any(|segment| segment == "..") {
        return None;
    }
    let mut path = raw.to_string();
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    Some(path)
}

/// One Keychain login's non-secret metadata. There is deliberately no password
/// field: candidate matching and UI never need the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedLoginMeta {
    /// Opaque native identifier (a session index or Keychain persistent ref
    /// digest), never a password.
    pub id: u64,
    pub origin: Origin,
    pub username: String,
    /// Keychain path hint, normalized like a page path.
    pub path: String,
    /// Ranking hint only; Unix seconds or another monotone native counter.
    pub last_used: i64,
}

impl SavedLoginMeta {
    pub fn new(id: u64, origin: Origin, username: impl Into<String>, path: &str, last_used: i64) -> Self {
        Self {
            id,
            origin,
            username: username.into(),
            path: normalize_path(path).unwrap_or_else(|| "/".to_string()),
            last_used,
        }
    }
}

/// A username offered to the user. The password stays in the Keychain until a
/// trusted fill is confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginCandidate {
    pub id: u64,
    pub username: String,
}

/// Match saved logins for `origin` and rank them. The origin must match
/// exactly; the path only orders candidates (exact, prefix, site-wide, other).
/// Duplicate usernames, duplicate ids, and unprintable usernames are skipped.
pub fn match_candidates(origin: &Origin, page_path: &str, logins: &[SavedLoginMeta], limit: usize) -> Vec<LoginCandidate> {
    if !origin.is_secure() || limit == 0 {
        return Vec::new();
    }
    let Some(page) = normalize_path(page_path) else {
        return Vec::new();
    };
    let mut ranked: Vec<(u8, i64, &SavedLoginMeta)> = logins
        .iter()
        .filter(|login| login.origin == *origin && username_is_displayable(&login.username))
        .filter_map(|login| normalize_path(&login.path).map(|path| (path_rank(&path, &page), login.last_used, login)))
        .collect();
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.username.to_lowercase().cmp(&b.2.username.to_lowercase()))
            .then_with(|| a.2.id.cmp(&b.2.id))
    });
    let mut out = Vec::new();
    let mut seen_usernames = HashSet::new();
    let mut seen_ids = HashSet::new();
    for (_, _, login) in ranked {
        if !seen_ids.insert(login.id) || !seen_usernames.insert(login.username.clone()) {
            continue;
        }
        out.push(LoginCandidate { id: login.id, username: login.username.clone() });
        if out.len() >= limit.min(MAX_CANDIDATES) {
            break;
        }
    }
    out
}

fn path_rank(item: &str, page: &str) -> u8 {
    if item == page {
        0
    } else if item == "/" {
        2
    } else if page.starts_with(item) && page.as_bytes().get(item.len()) == Some(&b'/') {
        1
    } else {
        3
    }
}

fn username_is_displayable(username: &str) -> bool {
    username.len() <= MAX_USERNAME_LEN && !username.chars().any(char::is_control)
}

/// A password value. `Debug` never reveals it; there is no `Serialize` and no
/// `Clone`, so an accidental log or copy is harder than with a bare `String`.
pub struct Password(String);

impl Password {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Reveals the secret to the native caller. Only the Keychain writer and
    /// the isolated-world fill call may use this; never log, serialize, or put
    /// the result into a bridge message.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// One captured login, held in native memory only long enough to write the
/// Keychain or answer a save prompt. `Debug` redacts the password; the type has
/// no `Serialize`, no `Clone`, and no bridge representation.
pub struct CapturedLogin {
    origin: Origin,
    username: String,
    path: String,
    password: Password,
}

impl CapturedLogin {
    pub fn new(origin: Origin, username: String, path: &str, password: Password) -> Self {
        Self {
            origin,
            username,
            path: normalize_path(path).unwrap_or_else(|| "/".to_string()),
            password,
        }
    }

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// See [`Password::expose`]: native Keychain/fill code only.
    pub fn password(&self) -> &str {
        self.password.expose()
    }

    /// A capture is usable only with a displayable username, a non-empty
    /// password, and a bounded password. Empty usernames are allowed: some
    /// sites have password-only forms, and the Keychain accepts an empty
    /// account.
    pub fn is_usable(&self) -> bool {
        username_is_displayable(&self.username) && !self.password.is_empty() && self.password.len() <= MAX_PASSWORD_LEN
    }
}

impl fmt::Debug for CapturedLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturedLogin")
            .field("origin", &self.origin)
            .field("username", &self.username)
            .field("path", &self.path)
            .field("password", &self.password)
            .finish()
    }
}

/// Native identity of one form instance. All three parts are minted natively
/// (a tab id, a navigation nonce, and a random form nonce); page-supplied ids
/// never reach this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FormKey {
    pub tab: u64,
    pub navigation: u64,
    pub form: u64,
}

impl FormKey {
    pub const fn new(tab: u64, navigation: u64, form: u64) -> Self {
        Self { tab, navigation, form }
    }
}

/// The save prompt's state for one form. Terminal states refuse every later
/// event, so a declined or failed form is never offered again in the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveState {
    Idle,
    AwaitingNavigation,
    Offer,
    Saving,
    Duplicate,
    Saved,
    Declined,
    Failed,
}

impl SaveState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Saved | Self::Declined | Self::Failed)
    }
}

/// A bounded reason for a failed Keychain write. Never includes a secret or a
/// raw platform error that could echo one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFailure {
    Denied,
    Unavailable,
    InvalidItem,
    Other,
}

/// Events the native layer may report after a trusted capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveEvent {
    /// The main-frame navigation settled at `destination`. The model compares
    /// it with the captured origin itself, so a caller cannot assert
    /// same-origin for a cross-origin navigation.
    Committed {
        destination: Origin,
        success_response: bool,
        password_field_present: bool,
    },
    NavigationFailed,
    Timeout,
    /// The user clicked the explicit Save button.
    Accept,
    Decline,
    /// Esc, click-away, or tab close counts as a decline.
    Dismiss,
    /// The user chose to replace an existing Keychain item.
    Replace,
    KeepExisting,
    KeychainSaved,
    KeychainFailed(SaveFailure),
}

/// What the native layer must do next. `WriteKeychain` takes ownership of the
/// captured secret; the state machine holds no copy while the write runs.
#[derive(Debug)]
pub enum SaveEffect {
    Ignored,
    Captured,
    PromptSave,
    PromptDuplicate,
    WriteKeychain { login: CapturedLogin, replace: bool },
    ShowError(SaveFailure),
    Discarded,
    Saved,
}

#[derive(Debug)]
pub struct SaveOutcome {
    pub state: SaveState,
    pub effect: SaveEffect,
}

/// The save/decline machine for one [`FormKey`]. It holds the captured login
/// only between a trusted submit and the explicit save decision, and drops it
/// on every refusal, failure, decline, timeout, or terminal state.
#[derive(Debug)]
pub struct FormSave {
    key: FormKey,
    state: SaveState,
    pending: Option<CapturedLogin>,
}

impl FormSave {
    pub fn new(key: FormKey) -> Self {
        Self { key, state: SaveState::Idle, pending: None }
    }

    pub fn key(&self) -> FormKey {
        self.key
    }

    pub fn state(&self) -> SaveState {
        self.state
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Capture after a trusted user submit. Untrusted (synthetic) submits,
    /// captures whose origin differs from the form's page, unusable captures,
    /// and a second capture for the same form are discarded without storing
    /// anything.
    pub fn capture(&mut self, trusted: bool, page_origin: &Origin, login: CapturedLogin) -> SaveOutcome {
        if self.state != SaveState::Idle
            || !trusted
            || login.origin() != page_origin
            || !page_origin.is_secure()
            || !login.is_usable()
        {
            return self.out(SaveEffect::Discarded);
        }
        self.pending = Some(login);
        self.state = SaveState::AwaitingNavigation;
        self.out(SaveEffect::Captured)
    }

    /// Report a duplicate found before writing. The native writer gives the
    /// captured login back so the user's Replace choice can update the existing
    /// item in place; Keep Existing stores nothing.
    pub fn duplicate_found(&mut self, login: CapturedLogin) -> SaveOutcome {
        if self.state != SaveState::Saving {
            return self.out(SaveEffect::Discarded);
        }
        self.pending = Some(login);
        self.state = SaveState::Duplicate;
        self.out(SaveEffect::PromptDuplicate)
    }

    pub fn on(&mut self, event: SaveEvent) -> SaveOutcome {
        match (self.state, event) {
            (
                SaveState::AwaitingNavigation,
                SaveEvent::Committed { destination, success_response, password_field_present },
            ) => {
                let same_origin = self.pending.as_ref().is_some_and(|login| login.origin() == &destination);
                if !same_origin || !destination.is_secure() || !success_response || password_field_present {
                    self.discard();
                    self.state = SaveState::Idle;
                    self.out(SaveEffect::Discarded)
                } else {
                    self.state = SaveState::Offer;
                    self.out(SaveEffect::PromptSave)
                }
            }
            (SaveState::AwaitingNavigation | SaveState::Offer, SaveEvent::NavigationFailed | SaveEvent::Timeout) => {
                self.discard();
                self.state = if self.state == SaveState::Offer { SaveState::Declined } else { SaveState::Idle };
                self.out(SaveEffect::Discarded)
            }
            (SaveState::Offer, SaveEvent::Accept) => match self.pending.take() {
                Some(login) => {
                    self.state = SaveState::Saving;
                    self.out(SaveEffect::WriteKeychain { login, replace: false })
                }
                None => {
                    self.state = SaveState::Failed;
                    self.out(SaveEffect::ShowError(SaveFailure::Other))
                }
            },
            (SaveState::Offer, SaveEvent::Decline | SaveEvent::Dismiss) => {
                self.discard();
                self.state = SaveState::Declined;
                self.out(SaveEffect::Discarded)
            }
            (SaveState::Saving, SaveEvent::KeychainSaved) => {
                self.state = SaveState::Saved;
                self.out(SaveEffect::Saved)
            }
            (SaveState::Saving, SaveEvent::KeychainFailed(failure)) => {
                self.state = SaveState::Failed;
                self.out(SaveEffect::ShowError(failure))
            }
            (SaveState::Duplicate, SaveEvent::Replace) => match self.pending.take() {
                Some(login) => {
                    self.state = SaveState::Saving;
                    self.out(SaveEffect::WriteKeychain { login, replace: true })
                }
                None => {
                    self.state = SaveState::Failed;
                    self.out(SaveEffect::ShowError(SaveFailure::Other))
                }
            },
            (SaveState::Duplicate, SaveEvent::KeepExisting | SaveEvent::Decline | SaveEvent::Dismiss) => {
                self.discard();
                self.state = SaveState::Declined;
                self.out(SaveEffect::Discarded)
            }
            _ => self.out(SaveEffect::Ignored),
        }
    }

    fn discard(&mut self) {
        self.pending = None;
    }

    fn out(&self, effect: SaveEffect) -> SaveOutcome {
        SaveOutcome { state: self.state, effect }
    }
}

/// How a fill was requested. `Trusted` is minted only by a native click or
/// Return handler; page-generated focus/submit events are `Synthetic` and are
/// refused. The bridge and MCP have no fill command at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Trusted,
    Synthetic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginField {
    Username,
    Password,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FillRefusal {
    #[error("synthetic events cannot start or confirm a fill")]
    SyntheticEvent,
    #[error("origin is not secure")]
    InsecureOrigin,
    #[error("origin changed since the offer")]
    StaleOrigin,
    #[error("form changed since the offer")]
    StaleForm,
    #[error("a login must be chosen explicitly")]
    NoSelection,
    #[error("the chosen login is no longer offered")]
    UnknownCandidate,
}

/// The candidate list shown by the native UI. It contains usernames only; a
/// password is fetched from the Keychain only after [`FillOffer::confirm`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillOffer {
    origin: Origin,
    form: FormKey,
    field: LoginField,
    candidates: Vec<LoginCandidate>,
}

impl FillOffer {
    /// Offer matching logins after a trusted focus/click on a login field.
    /// Returns `Ok(None)` when the origin has no saved login. A synthetic
    /// gesture never creates an offer.
    pub fn open(
        origin: Origin,
        form: FormKey,
        field: LoginField,
        gesture: Gesture,
        page_path: &str,
        logins: &[SavedLoginMeta],
        limit: usize,
    ) -> Result<Option<Self>, FillRefusal> {
        if gesture != Gesture::Trusted {
            return Err(FillRefusal::SyntheticEvent);
        }
        if !origin.is_secure() {
            return Err(FillRefusal::InsecureOrigin);
        }
        let candidates = match_candidates(&origin, page_path, logins, limit);
        if candidates.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self { origin, form, field, candidates }))
    }

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    pub fn form(&self) -> FormKey {
        self.form
    }

    pub fn field(&self) -> LoginField {
        self.field
    }

    pub fn candidates(&self) -> &[LoginCandidate] {
        &self.candidates
    }

    /// Confirm an explicit user choice. The origin and form must still be the
    /// same ones the offer was built for, and a choice is mandatory, so there
    /// is no path to a silent or automatic fill.
    pub fn confirm(
        &self,
        selection: Option<usize>,
        gesture: Gesture,
        current_origin: &Origin,
        current_form: FormKey,
    ) -> Result<FillTarget, FillRefusal> {
        if gesture != Gesture::Trusted {
            return Err(FillRefusal::SyntheticEvent);
        }
        if current_origin != &self.origin {
            return Err(FillRefusal::StaleOrigin);
        }
        if current_form != self.form {
            return Err(FillRefusal::StaleForm);
        }
        let index = selection.ok_or(FillRefusal::NoSelection)?;
        let candidate = self.candidates.get(index).ok_or(FillRefusal::UnknownCandidate)?.clone();
        Ok(FillTarget { origin: self.origin.clone(), form: self.form, candidate })
    }
}

/// What the native layer may pass to the isolated-world fill script. It names
/// the login; the password is resolved from the Keychain at fill time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillTarget {
    pub origin: Origin,
    pub form: FormKey,
    pub candidate: LoginCandidate,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(url: &str) -> Origin {
        Origin::parse(url).unwrap()
    }

    fn captured(url: &str, username: &str, path: &str, password: &str) -> CapturedLogin {
        CapturedLogin::new(origin(url), username.to_string(), path, Password::new(password))
    }

    fn meta(id: u64, url: &str, username: &str, path: &str, last_used: i64) -> SavedLoginMeta {
        SavedLoginMeta::new(id, origin(url), username, path, last_used)
    }

    fn commit(url: &str) -> SaveEvent {
        SaveEvent::Committed { destination: origin(url), success_response: true, password_field_present: false }
    }

    fn commit_result(url: &str, success_response: bool, password_field_present: bool) -> SaveEvent {
        SaveEvent::Committed { destination: origin(url), success_response, password_field_present }
    }

    #[test]
    fn origin_normalizes_scheme_host_and_default_ports() {
        let a = origin("HTTPS://Example.COM:443/login?x=1#frag");
        assert_eq!(a.to_string(), "https://example.com");
        assert_eq!(a.host(), "example.com");
        assert_eq!(a.port(), None);
        assert_eq!(a.effective_port(), 443);
        assert!(a.is_https() && a.is_secure() && !a.is_loopback());

        let b = origin("https://example.com:8443");
        assert_eq!(b.to_string(), "https://example.com:8443");
        assert_eq!(b.port(), Some(8443));
        assert_eq!(b.effective_port(), 8443);
        assert_ne!(a, b);

        assert_eq!(origin("http://localhost:80").to_string(), "http://localhost");
        assert_eq!(origin("http://127.0.0.1").to_string(), "http://127.0.0.1");
        assert_eq!(origin("http://[::1]:8080").to_string(), "http://[::1]:8080");
        assert_eq!(origin("https://[::1]").to_string(), "https://[::1]");
    }

    #[test]
    fn origin_refuses_insecure_non_loopback_and_embedded_credentials() {
        assert_eq!(Origin::parse("http://example.com"), Err(OriginError::Insecure));
        assert_eq!(Origin::parse("http://192.168.1.1:8080"), Err(OriginError::Insecure));
        assert_eq!(Origin::parse("https://user:hunter2@example.com"), Err(OriginError::UserInfo));
        assert_eq!(Origin::parse("https://example.com@evil.test"), Err(OriginError::UserInfo));
        assert_eq!(Origin::parse("file:///tmp/x"), Err(OriginError::NotAbsolute));
        assert_eq!(Origin::parse("javascript:alert(1)"), Err(OriginError::NotAbsolute));
        assert_eq!(Origin::parse(""), Err(OriginError::Empty));

        let error = Origin::parse("https://user:hunter2@example.com").unwrap_err();
        assert!(!format!("{error}").contains("hunter2"), "an origin error never echoes the URL");
    }

    #[test]
    fn origin_refuses_malformed_hosts_and_ports() {
        assert_eq!(Origin::parse("https://exa mple.com"), Err(OriginError::NotAbsolute));
        assert_eq!(Origin::parse("https://example.com."), Err(OriginError::Host));
        assert_eq!(Origin::parse("https://a..b"), Err(OriginError::Host));
        assert_eq!(Origin::parse("https://-bad.test"), Err(OriginError::Host));
        assert_eq!(Origin::parse("https://bad-.test"), Err(OriginError::Host));
        assert_eq!(Origin::parse("https://example.com:0"), Err(OriginError::Port));
        assert_eq!(Origin::parse("https://example.com:65536"), Err(OriginError::Port));
        assert_eq!(Origin::parse("https://example.com:https"), Err(OriginError::Port));
        assert_eq!(Origin::parse("https://bücher.example"), Err(OriginError::Host));
        // Percent-encoded dots are a path, not a traversal: the page path is
        // only a ranking hint and the origin is still exact.
        assert_eq!(
            Origin::parse("https://example.com/%2e%2e/").unwrap(),
            origin("https://example.com")
        );
    }

    #[test]
    fn origin_paths_are_normalized_for_ranking_only() {
        let (o, path) = Origin::parse_with_path("https://example.com/login/?next=1#frag").unwrap();
        assert_eq!(o.to_string(), "https://example.com");
        assert_eq!(path, "/login");
        assert_eq!(Origin::parse_with_path("https://example.com").unwrap().1, "/");
        assert_eq!(Origin::parse_with_path("https://example.com?x=1").unwrap().1, "/");
        assert_eq!(Origin::parse_with_path("https://example.com/a//b/").unwrap().1, "/a//b");
        assert_eq!(Origin::parse_with_path("https://example.com/../etc").unwrap_err(), OriginError::Path);
        assert_eq!(Origin::parse_with_path("https://example.com/%2e%2e/etc").unwrap().1, "/%2e%2e/etc");
    }

    #[test]
    fn matching_is_exact_origin_and_ranks_path_then_recency() {
        let page = origin("https://example.com");
        let logins = vec![
            meta(1, "https://example.com", "alice", "/login", 10),
            meta(2, "https://example.com", "bob", "/", 20),
            meta(3, "https://example.com", "alice", "/", 30),
            meta(4, "https://www.example.com", "eve", "/", 99),
            meta(5, "https://other.test", "mallory", "/", 99),
            meta(6, "https://example.com", "carol", "/admin", 5),
        ];
        let got = match_candidates(&page, "/login", &logins, 5);
        assert_eq!(got.iter().map(|c| c.username.as_str()).collect::<Vec<_>>(), ["alice", "bob", "carol"]);
        assert_eq!(got[0].id, 1, "the exact-path item outranks the same username's site-wide item");

        let prefix = match_candidates(&page, "/login/step2", &logins, 5);
        assert_eq!(prefix.iter().map(|c| c.username.as_str()).collect::<Vec<_>>(), ["alice", "bob", "carol"]);

        assert!(match_candidates(&page, "/", &logins, 5).iter().all(|c| c.username != "eve"));
        assert!(match_candidates(&page, "/../bad", &logins, 5).is_empty());
    }

    #[test]
    fn matching_dedups_caps_and_skips_unprintable_usernames() {
        let page = origin("https://example.com");
        let logins = vec![
            meta(1, "https://example.com", "alice", "/", 1),
            meta(2, "https://example.com", "alice", "/", 2),
            meta(3, "https://example.com", "bob", "/", 3),
            meta(4, "https://example.com", "carol", "/", 4),
            meta(5, "https://example.com", "dave", "/", 5),
            meta(6, "https://example.com", "eve", "/", 6),
            meta(7, "https://example.com", "line\nbreak", "/", 7),
        ];
        let got = match_candidates(&page, "/", &logins, 99);
        assert_eq!(got.len(), MAX_CANDIDATES);
        assert_eq!(got.iter().filter(|c| c.username == "alice").count(), 1);
        assert!(got.iter().all(|c| !c.username.contains('\n')));
        assert!(match_candidates(&page, "/", &logins, 0).is_empty());
    }

    #[test]
    fn save_prompts_only_after_a_successful_same_origin_navigation() {
        let mut save = FormSave::new(FormKey::new(1, 2, 3));
        let outcome = save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
        assert!(matches!(outcome.effect, SaveEffect::Captured));
        assert!(save.has_pending());

        let outcome = save.on(commit("https://example.com/login/step2"));
        assert_eq!(outcome.state, SaveState::Offer);
        assert!(matches!(outcome.effect, SaveEffect::PromptSave));
    }

    #[test]
    fn failed_or_cross_origin_logins_drop_the_capture() {
        for committed in [
            commit("https://other.test"),
            commit_result("https://example.com", false, false),
            commit_result("https://example.com", true, true),
        ] {
            let mut save = FormSave::new(FormKey::new(1, 2, 3));
            save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
            let outcome = save.on(committed);
            assert_eq!(outcome.state, SaveState::Idle, "{outcome:?}");
            assert!(!save.has_pending(), "{outcome:?}");
            assert!(matches!(outcome.effect, SaveEffect::Discarded));
        }

        let mut save = FormSave::new(FormKey::new(1, 2, 3));
        save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
        let outcome = save.on(SaveEvent::NavigationFailed);
        assert_eq!(outcome.state, SaveState::Idle);
        assert!(!save.has_pending());
    }

    #[test]
    fn decline_drops_the_secret_and_is_never_repeated() {
        let mut save = FormSave::new(FormKey::new(1, 2, 3));
        save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
        save.on(commit("https://example.com"));
        let outcome = save.on(SaveEvent::Decline);
        assert_eq!(outcome.state, SaveState::Declined);
        assert!(!save.has_pending());

        let again = save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
        assert_eq!(again.state, SaveState::Declined);
        assert!(matches!(again.effect, SaveEffect::Discarded));
        assert!(matches!(save.on(SaveEvent::Accept).effect, SaveEffect::Ignored));
    }

    #[test]
    fn save_accept_moves_the_secret_out_and_duplicate_replace_keeps_it_until_decided() {
        let mut save = FormSave::new(FormKey::new(1, 2, 3));
        save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
        save.on(commit("https://example.com"));

        let login = match save.on(SaveEvent::Accept).effect {
            SaveEffect::WriteKeychain { login, replace } => {
                assert!(!replace);
                login
            }
            other => panic!("expected a write, got {other:?}"),
        };
        assert_eq!(save.state(), SaveState::Saving);
        assert!(!save.has_pending(), "the writer owns the only copy");

        let outcome = save.duplicate_found(login);
        assert_eq!(outcome.state, SaveState::Duplicate);
        assert!(save.has_pending());
        assert!(matches!(outcome.effect, SaveEffect::PromptDuplicate));

        let login = match save.on(SaveEvent::Replace).effect {
            SaveEffect::WriteKeychain { login, replace } => {
                assert!(replace);
                login
            }
            other => panic!("expected a replacement write, got {other:?}"),
        };
        assert_eq!(login.username(), "alice");
        assert_eq!(save.state(), SaveState::Saving);

        let outcome = save.on(SaveEvent::KeychainSaved);
        assert_eq!(outcome.state, SaveState::Saved);
        assert!(matches!(outcome.effect, SaveEffect::Saved));
        assert!(!save.has_pending());
    }

    #[test]
    fn keep_existing_and_keychain_failure_store_nothing_and_end_the_form() {
        let mut save = FormSave::new(FormKey::new(1, 2, 3));
        save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
        save.on(commit("https://example.com"));
        let login = match save.on(SaveEvent::Accept).effect {
            SaveEffect::WriteKeychain { login, .. } => login,
            other => panic!("expected a write, got {other:?}"),
        };
        save.duplicate_found(login);
        let outcome = save.on(SaveEvent::KeepExisting);
        assert_eq!(outcome.state, SaveState::Declined);
        assert!(!save.has_pending());

        let mut save = FormSave::new(FormKey::new(4, 5, 6));
        save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
        save.on(commit("https://example.com"));
        let _ = save.on(SaveEvent::Accept);
        let outcome = save.on(SaveEvent::KeychainFailed(SaveFailure::Denied));
        assert_eq!(outcome.state, SaveState::Failed);
        assert!(matches!(outcome.effect, SaveEffect::ShowError(SaveFailure::Denied)));
        assert!(!save.has_pending());
        assert!(save.on(SaveEvent::KeychainSaved).state == SaveState::Failed, "a failed write is terminal");
    }

    #[test]
    fn synthetic_or_cross_origin_captures_are_discarded() {
        let mut save = FormSave::new(FormKey::new(1, 2, 3));
        let outcome = save.capture(false, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
        assert!(matches!(outcome.effect, SaveEffect::Discarded));
        assert_eq!(outcome.state, SaveState::Idle);
        assert!(!save.has_pending());

        let outcome = save.capture(true, &origin("https://other.test"), captured("https://example.com", "alice", "/login", "hunter2"));
        assert!(matches!(outcome.effect, SaveEffect::Discarded));
        assert!(!save.has_pending());

        let outcome = save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", ""));
        assert!(matches!(outcome.effect, SaveEffect::Discarded));
        assert!(!save.has_pending());

        let outcome =
            save.capture(true, &origin("https://example.com"), captured("https://example.com", "bad\nname", "/login", "hunter2"));
        assert!(matches!(outcome.effect, SaveEffect::Discarded));
    }

    #[test]
    fn timeout_or_late_duplicate_never_leaves_a_pending_secret() {
        let mut save = FormSave::new(FormKey::new(1, 2, 3));
        save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
        let outcome = save.on(SaveEvent::Timeout);
        assert_eq!(outcome.state, SaveState::Idle);
        assert!(!save.has_pending());

        save.capture(true, &origin("https://example.com"), captured("https://example.com", "alice", "/login", "hunter2"));
        save.on(commit("https://example.com"));
        let outcome = save.on(SaveEvent::Timeout);
        assert_eq!(outcome.state, SaveState::Declined);
        assert!(!save.has_pending());

        let late = save.duplicate_found(captured("https://example.com", "alice", "/login", "hunter2"));
        assert!(matches!(late.effect, SaveEffect::Discarded));
        assert_eq!(late.state, SaveState::Declined);
        assert!(!save.has_pending());
    }

    #[test]
    fn debug_output_never_contains_a_password() {
        let password = Password::new("hunter2");
        assert_eq!(format!("{password:?}"), "<redacted>");

        let login = captured("https://example.com", "alice", "/login", "hunter2");
        let debug = format!("{login:?}");
        assert!(debug.contains("alice"));
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("hunter2"), "{debug}");

        let mut save = FormSave::new(FormKey::new(1, 2, 3));
        save.capture(true, &origin("https://example.com"), login);
        save.on(commit("https://example.com"));
        let outcome = save.on(SaveEvent::Accept);
        assert!(!format!("{outcome:?}").contains("hunter2"), "a save effect never prints the secret");
    }

    #[test]
    fn fill_requires_a_trusted_gesture_and_an_explicit_current_choice() {
        let page = origin("https://example.com");
        let form = FormKey::new(1, 2, 3);
        let logins = vec![
            meta(1, "https://example.com", "alice", "/login", 10),
            meta(2, "https://example.com", "bob", "/", 20),
        ];

        assert_eq!(
            FillOffer::open(page.clone(), form, LoginField::Password, Gesture::Synthetic, "/login", &logins, 5),
            Err(FillRefusal::SyntheticEvent)
        );
        assert_eq!(
            FillOffer::open(page.clone(), form, LoginField::Username, Gesture::Trusted, "/login", &[], 5).unwrap(),
            None
        );

        let offer = FillOffer::open(page.clone(), form, LoginField::Password, Gesture::Trusted, "/login", &logins, 5)
            .unwrap()
            .unwrap();
        assert_eq!(offer.candidates().len(), 2);
        assert_eq!(offer.field(), LoginField::Password);

        assert_eq!(offer.confirm(None, Gesture::Trusted, &page, form), Err(FillRefusal::NoSelection));
        assert_eq!(offer.confirm(Some(0), Gesture::Synthetic, &page, form), Err(FillRefusal::SyntheticEvent));
        assert_eq!(offer.confirm(Some(0), Gesture::Trusted, &origin("https://other.test"), form), Err(FillRefusal::StaleOrigin));
        assert_eq!(
            offer.confirm(Some(0), Gesture::Trusted, &page, FormKey::new(1, 2, 99)),
            Err(FillRefusal::StaleForm)
        );
        assert_eq!(offer.confirm(Some(99), Gesture::Trusted, &page, form), Err(FillRefusal::UnknownCandidate));

        let target = offer.confirm(Some(0), Gesture::Trusted, &page, form).unwrap();
        assert_eq!(target.origin, page);
        assert_eq!(target.candidate.username, "alice");
        assert_eq!(target.form, form);
    }
}
