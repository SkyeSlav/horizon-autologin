//! Auto Login & Auto Register for Horizon: detects login/register prompts in
//! incoming chat and logs you in with the saved password for that server, or
//! registers you with a generated password and saves it.
//!
//! Pure logic: on join, a saved password logs in with no prompt needed, once consent is given for that server;
//! the hooks in `lib.rs` call [`note_chat`] for every chat line,
//! [`tick`] once per client tick on a server, and [`command`] for the mod's
//! own commands. All state lives behind one mutex; the hooks do the game's
//! I/O (settings, data files, sending commands, chat lines) and hand values
//! in through [`load`] / [`take_store`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ID: &str = "autologin";
pub const NAME: &str = "Auto Login";
pub const VERSION: &str = "1.1.6";
pub const DESCRIPTION: &str = "Logs you in on join when a password is saved and consent is given, and watches for login/register prompts in chat, in the action bar and on the boss bar, per server.";

/// Settings keys, one file per key under the mod's data folder.
const KEY_LOGIN_PATTERN: &str = "login_pattern";
const KEY_REGISTER_PATTERN: &str = "register_pattern";
const KEY_AUTODETECT_LOGIN: &str = "autodetect_login";
const KEY_AUTODETECT_REGISTER: &str = "autodetect_register";
const KEY_USE_DELAY: &str = "use_delay";
const KEY_DELAY_SECONDS: &str = "delay_seconds";
const KEY_LOGIN_TEMPLATE: &str = "login_template";
const KEY_REGISTER_TEMPLATE: &str = "register_template";
const KEY_PASSWORD_LENGTH: &str = "password_length";
const KEY_PASSWORD_CHARS: &str = "password_chars";
const KEY_DISABLE_ONCE_LOGGED_IN: &str = "disable_once_logged_in";
const KEY_DISABLE_ONCE_REGISTERED: &str = "disable_once_registered";
const KEY_SHOW_DEBUG: &str = "show_debug";
const KEY_AUTOSAVE: &str = "autosave";
const KEY_PASSWORDS: &str = "passwords";
const KEY_CONSENTS: &str = "consents";

/// A per-server password, stored as one `server|player|password` line.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Password {
    server: String,
    player: String,
    password: String,
}

/// The mod's settings.
#[derive(Clone, Debug)]
pub struct Config {
    pub login_pattern: String,
    pub register_pattern: String,
    pub autodetect_login: bool,
    pub autodetect_register: bool,
    pub use_delay: bool,
    pub delay_seconds: u64,
    pub login_template: String,
    pub register_template: String,
    pub password_length: usize,
    pub password_chars: String,
    pub disable_once_logged_in: bool,
    pub disable_once_registered: bool,
    pub show_debug: bool,
    pub autosave: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            login_pattern: "login".to_string(),
            register_pattern: "register".to_string(),
            autodetect_login: true,
            autodetect_register: true,
            use_delay: true,
            delay_seconds: 1,
            login_template: "/login $p".to_string(),
            register_template: "/register $p $p".to_string(),
            password_length: 6,
            password_chars: "abcdefghijklmnopqrstuvwxyz0123456789".to_string(),
            disable_once_logged_in: true,
            disable_once_registered: true,
            show_debug: false,
            autosave: true,
        }
    }
}

impl Config {
    fn load(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let bool = |k: &str, default: bool| match get(k).as_deref() {
            Some("true") => true,
            Some("false") => false,
            _ => default,
        };
        let num = |k: &str, default: u64| {
            get(k)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(default)
        };
        let text = |k: &str, default: &str| {
            get(k)
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        let length = num(KEY_PASSWORD_LENGTH, d.password_length as u64);
        Self {
            login_pattern: text(KEY_LOGIN_PATTERN, &d.login_pattern),
            register_pattern: text(KEY_REGISTER_PATTERN, &d.register_pattern),
            autodetect_login: bool(KEY_AUTODETECT_LOGIN, d.autodetect_login),
            autodetect_register: bool(KEY_AUTODETECT_REGISTER, d.autodetect_register),
            use_delay: bool(KEY_USE_DELAY, d.use_delay),
            delay_seconds: num(KEY_DELAY_SECONDS, d.delay_seconds).min(30),
            login_template: text(KEY_LOGIN_TEMPLATE, &d.login_template),
            register_template: text(KEY_REGISTER_TEMPLATE, &d.register_template),
            password_length: if length == 0 {
                6
            } else {
                length.min(64) as usize
            },
            password_chars: text(KEY_PASSWORD_CHARS, &d.password_chars),
            disable_once_logged_in: bool(KEY_DISABLE_ONCE_LOGGED_IN, d.disable_once_logged_in),
            disable_once_registered: bool(KEY_DISABLE_ONCE_REGISTERED, d.disable_once_registered),
            show_debug: bool(KEY_SHOW_DEBUG, d.show_debug),
            autosave: bool(KEY_AUTOSAVE, d.autosave),
        }
    }

    fn stored(&self) -> Vec<(&'static str, String)> {
        vec![
            (KEY_LOGIN_PATTERN, self.login_pattern.clone()),
            (KEY_REGISTER_PATTERN, self.register_pattern.clone()),
            (KEY_AUTODETECT_LOGIN, self.autodetect_login.to_string()),
            (
                KEY_AUTODETECT_REGISTER,
                self.autodetect_register.to_string(),
            ),
            (KEY_USE_DELAY, self.use_delay.to_string()),
            (KEY_DELAY_SECONDS, self.delay_seconds.to_string()),
            (KEY_LOGIN_TEMPLATE, self.login_template.clone()),
            (KEY_REGISTER_TEMPLATE, self.register_template.clone()),
            (KEY_PASSWORD_LENGTH, self.password_length.to_string()),
            (KEY_PASSWORD_CHARS, self.password_chars.clone()),
            (
                KEY_DISABLE_ONCE_LOGGED_IN,
                self.disable_once_logged_in.to_string(),
            ),
            (
                KEY_DISABLE_ONCE_REGISTERED,
                self.disable_once_registered.to_string(),
            ),
            (KEY_SHOW_DEBUG, self.show_debug.to_string()),
            (KEY_AUTOSAVE, self.autosave.to_string()),
        ]
    }
}

/// A login or register the delay is counting down for. The command is
/// rendered at dispatch, not at schedule time, so the toggle and the
/// saved password are rechecked with their current values.
struct Pending {
    login: bool,
    password: String,
    template: String,
    due: u64,
    /// The tick of the prompt that scheduled it; prompts up to here are spent.
    stamp: u64,
}

struct State {
    config: Config,
    passwords: Vec<Password>,
    /// (server, player) pairs auto-login and auto-register are allowed for.
    /// Consent is recorded once per server by an explicit player act and
    /// survives restarts; a saved password without it stays inactive.
    consents: Vec<(String, String)>,
    /// A password `/autologin set` staged, waiting for the second line:
    /// (server, player, password). Memory only, never written to a file.
    staged: Option<(String, String, String)>,
    /// The consent notice already shown on this connection, per kind.
    asked_legacy: bool,
    asked_register: bool,
    /// Settings writes the adapter still has to persist.
    store: Vec<(String, String)>,
    tick: u64,
    current: Option<(String, String)>,
    current_since: u64,
    attempted: bool,
    /// The promptless login already went out on this connection.
    join_login_done: bool,
    pending: Option<Pending>,
    /// (tick, server, player, text), oldest first, capped.
    recent: Vec<(u64, String, String, String)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            config: Config::default(),
            passwords: Vec::new(),
            consents: Vec::new(),
            staged: None,
            asked_legacy: false,
            asked_register: false,
            store: Vec::new(),
            tick: 0,
            current: None,
            current_since: 0,
            attempted: false,
            join_login_done: false,
            pending: None,
            recent: Vec::new(),
        }
    }
}

impl State {
    fn password(&self, server: &str, player: &str) -> Option<&str> {
        self.passwords
            .iter()
            .find(|p| p.server == server && p.player == player)
            .map(|p| p.password.as_str())
    }

    fn set_password(&mut self, server: &str, player: &str, password: &str, persist: bool) {
        if let Some(p) = self
            .passwords
            .iter_mut()
            .find(|p| p.server == server && p.player == player)
        {
            p.password = password.to_string();
        } else {
            self.passwords.push(Password {
                server: server.to_string(),
                player: player.to_string(),
                password: password.to_string(),
            });
        }
        if persist {
            self.queue_passwords();
        }
    }

    fn queue_passwords(&mut self) {
        let encoded = self
            .passwords
            .iter()
            .map(|p| format!("{}|{}|{}", p.server, p.player, p.password))
            .collect::<Vec<_>>()
            .join("\n");
        self.store.push((KEY_PASSWORDS.to_string(), encoded));
    }

    fn has_consent(&self, server: &str, player: &str) -> bool {
        self.consents
            .iter()
            .any(|(s, p)| s == server && p == player)
    }

    /// Record consent for (server, player). Queues a store write only
    /// when it is new.
    fn grant_consent(&mut self, server: &str, player: &str) {
        if !self.has_consent(server, player) {
            self.consents.push((server.to_string(), player.to_string()));
            self.queue_consents();
        }
    }

    fn queue_consents(&mut self) {
        let encoded = self
            .consents
            .iter()
            .map(|(s, p)| format!("{s}|{p}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.store.push((KEY_CONSENTS.to_string(), encoded));
    }

    fn queue_config(&mut self) {
        for (k, v) in self.config.stored() {
            self.store.push((k.to_string(), v));
        }
    }
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read the settings (and saved passwords) from the game's files.
pub fn load(get: &dyn Fn(&str) -> Option<String>) {
    let mut s = state();
    s.config = Config::load(get);
    s.passwords = get(KEY_PASSWORDS)
        .map(|v| {
            v.lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(3, '|');
                    Some(Password {
                        server: parts.next()?.to_string(),
                        player: parts.next()?.to_string(),
                        password: parts.next()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    s.consents = get(KEY_CONSENTS)
        .map(|v| {
            v.lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(2, '|');
                    Some((parts.next()?.to_string(), parts.next()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
}

/// Settings writes the adapter still has to persist.
pub fn take_store() -> Vec<(String, String)> {
    std::mem::take(&mut state().store)
}

/// Move a saved password from one player key to another (a name-keyed
/// entry to the account-UUID key), when the new key has none saved.
/// Queues a store write only when something moved.
pub fn migrate_password(server: &str, from_player: &str, to_player: &str) -> bool {
    if from_player == to_player {
        return false;
    }
    let mut s = state();
    if s.passwords
        .iter()
        .any(|p| p.server == server && p.player == to_player)
    {
        return false;
    }
    let Some(idx) = s
        .passwords
        .iter()
        .position(|p| p.server == server && p.player == from_player)
    else {
        return false;
    };
    s.passwords[idx].player = to_player.to_string();
    s.queue_passwords();
    true
}

/// A new login event: clear the previous session's prompts, queued sends
/// and consent asks, but NOT the attempt flags. The one-attempt limit
/// survives a proxy reconfiguration (a new login on a backend switch or
/// a transfer); only a real off-server tick — `tick(None)` — resets it,
/// which is what makes a genuine rejoin fire again.
pub fn new_connection() {
    let mut s = state();
    s.current = None;
    s.pending = None;
    s.recent.clear();
    s.staged = None;
    s.asked_legacy = false;
    s.asked_register = false;
}

/// A chat line arrived from `server` for `player`. Only its text is kept.
pub fn note_chat(server: &str, player: &str, text: &str) {
    let mut s = state();
    let tick = s.tick;
    s.recent.push((
        tick,
        server.to_string(),
        player.to_string(),
        text.to_string(),
    ));
    if s.recent.len() > 16 {
        let excess = s.recent.len() - 16;
        s.recent.drain(..excess);
    }
}

/// What a tick does: the command to send, if the delay ran out, and any
/// chat lines for the player.
#[derive(Default)]
pub struct TickOut {
    pub send: Option<String>,
    pub lines: Vec<String>,
}

/// One client tick on a server. `key` is the server address and player
/// identity, or None when not on a server.
pub fn tick(key: Option<(&str, &str)>) -> TickOut {
    let mut out = TickOut::default();
    let mut s = state();
    s.tick += 1;
    let Some((server, player)) = key else {
        // Off-server: the one-attempt limit resets here, and only here —
        // a mere address change mid-session (proxy reconfiguration) does
        // not.
        s.current = None;
        s.pending = None;
        s.recent.clear();
        s.staged = None;
        s.asked_legacy = false;
        s.asked_register = false;
        s.attempted = false;
        s.join_login_done = false;
        return out;
    };
    if s.current.as_ref().map(|(a, b)| (a.as_str(), b.as_str())) != Some((server, player)) {
        s.current = Some((server.to_string(), player.to_string()));
        // Prompts for this server noted just before the first tick of the
        // join still count; prompts from another server do not.
        s.recent.retain(|r| r.1 == server && r.2 == player);
        s.current_since = s.recent.iter().map(|r| r.0).min().unwrap_or(s.tick);
        // The attempt flags are NOT reset here: a new login on the same
        // session (a proxy backend switch or transfer) keeps the
        // one-attempt limit. A genuine rejoin was already reset by the
        // off-server tick.
        s.staged = None;
        s.asked_legacy = false;
        s.asked_register = false;
        s.pending = None;
    }
    if s.attempted {
        return out;
    }
    let cfg = s.config.clone();
    // A queued send is cancelled when its auto-detect toggle goes off.
    if let Some(p) = &s.pending {
        let on = if p.login {
            cfg.autodetect_login
        } else {
            cfg.autodetect_register
        };
        if !on {
            s.pending = None;
        }
    }
    // Schedule from the newest prompt for this server, if nothing is pending.
    if s.pending.is_none() {
        let since = s.current_since;
        let mut found: Option<(bool, u64)> = None;
        // A saved password logs in on join with no prompt needed — but
        // only once consent is recorded for this server. Without it, the
        // mod shows the consent notice once per connection and waits;
        // the saved password stays inactive until then. The auto-detect
        // toggle is still the off switch.
        let mut joined_with_password = false;
        let consented = s.has_consent(server, player);
        // The promptless login is consumed once per connection: without
        // this, a saved password would reschedule it every delay
        // interval once the once-per-join stop is off.
        if cfg.autodetect_login && !s.join_login_done && s.password(server, player).is_some() {
            if consented {
                found = Some((true, s.tick));
                joined_with_password = true;
            } else if !s.asked_legacy {
                s.asked_legacy = true;
                out.lines.push(format!(
                    "[AutoLogin] A saved password is stored for {server}. Allow auto-login here? /autologin allow"
                ));
            }
        }
        if found.is_none() {
            for (stamp, srv, ply, text) in s.recent.iter().rev() {
                if *stamp < since {
                    break;
                }
                if srv != server || ply != player {
                    continue;
                }
                let lower = text.to_lowercase();
                let login_hit =
                    cfg.autodetect_login && lower.contains(&cfg.login_pattern.to_lowercase());
                let register_hit =
                    cfg.autodetect_register && lower.contains(&cfg.register_pattern.to_lowercase());
                if login_hit && s.password(server, player).is_some() {
                    if consented {
                        found = Some((true, *stamp));
                    } else if !s.asked_legacy {
                        s.asked_legacy = true;
                        out.lines.push(format!(
                            "[AutoLogin] A saved password is stored for {server}. Allow auto-login here? /autologin allow"
                        ));
                    }
                    break;
                }
                if register_hit && s.password(server, player).is_none() {
                    if consented {
                        found = Some((false, *stamp));
                    } else if !s.asked_register {
                        s.asked_register = true;
                        out.lines.push(format!(
                            "[AutoLogin] This server asks you to register. Allow Auto Login to register and log in on {server}? /autologin allow"
                        ));
                    }
                    break;
                }
            }
        }
        if let Some((login, stamp)) = found {
            let (password, template) = if login {
                let pw = s.password(server, player).unwrap_or_default().to_string();
                (pw, cfg.login_template.clone())
            } else {
                // The generated password is kept pending in memory only
                // and saved at dispatch, so a cancelled registration
                // leaves no credential behind for an account that was
                // never created.
                let pw = generate_password(&cfg);
                (pw, cfg.register_template.clone())
            };
            let due = if cfg.use_delay {
                s.tick + cfg.delay_seconds.saturating_mul(20)
            } else {
                s.tick
            };
            if cfg.show_debug {
                out.lines.push(if joined_with_password {
                    format!(
                        "[AutoLogin] Saved password for {server}; logging in in {}s.",
                        if cfg.use_delay { cfg.delay_seconds } else { 0 },
                    )
                } else {
                    format!(
                        "[AutoLogin] Saw a {} prompt on {server}; sending in {}s.",
                        if login { "login" } else { "register" },
                        if cfg.use_delay { cfg.delay_seconds } else { 0 },
                    )
                });
            }
            s.pending = Some(Pending {
                login,
                password,
                template,
                due,
                stamp,
            });
        }
    }
    // Send when the delay ran out. The connection, the toggle and the
    // current credential are rechecked immediately before dispatch, so
    // a stale queued send never goes out.
    if let Some(p) = &s.pending {
        if s.tick >= p.due {
            let p = s.pending.take().unwrap();
            let on = if p.login {
                cfg.autodetect_login
            } else {
                cfg.autodetect_register
            };
            let same_connection =
                s.current.as_ref().map(|(a, b)| (a.as_str(), b.as_str())) == Some((server, player));
            // A login re-reads the saved password: a changed password is
            // sent, a deleted one cancels the send. A register saves its
            // generated password only now that it is actually sent; if a
            // password was saved meanwhile, the queued register is
            // dropped instead of overwriting it.
            let password = if p.login {
                s.password(server, player).map(|pw| pw.to_string())
            } else if s.password(server, player).is_some() {
                None
            } else {
                Some(p.password.clone())
            };
            if on && same_connection {
                if let Some(password) = password {
                    if p.login {
                        if cfg.disable_once_logged_in {
                            s.attempted = true;
                        }
                        s.join_login_done = true;
                    } else {
                        // The generated password is saved only now that
                        // the register is actually sent.
                        let persist = cfg.autosave;
                        s.set_password(server, player, &password, persist);
                        if cfg.show_debug && !persist {
                            out.lines.push(
                                "[AutoLogin] Registered with a generated password, but autosave is off: it will not survive a restart."
                                    .to_string(),
                            );
                        }
                        if cfg.disable_once_registered {
                            s.attempted = true;
                        }
                    }
                    // The prompt that scheduled this is spent, so it cannot fire
                    // again when the once-per-join stop is off.
                    s.recent.retain(|r| r.0 > p.stamp);
                    out.send = Some(render_template(&p.template, &password));
                }
            }
        }
    }
    out
}

/// Fill a command template (`/login $p`, `/register $p $p`) with the
/// password, and drop its leading slash: the game sends commands bare.
fn render_template(template: &str, password: &str) -> String {
    let rendered = template.replace("$p", password);
    let rendered = rendered.trim();
    rendered.strip_prefix('/').unwrap_or(rendered).to_string()
}

/// A password from the configured characters. xorshift64*, seeded from the
/// clock and a counter: good enough for a per-server game password, not a
/// cryptographic secret.
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn generate_password(cfg: &Config) -> String {
    let chars: Vec<u8> = if cfg.password_chars.is_empty() {
        b"abcdefghijklmnopqrstuvwxyz0123456789".to_vec()
    } else {
        cfg.password_chars.bytes().collect()
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut x = nanos
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ 0x2545_f491_4f6c_dd1d;
    if x == 0 {
        x = 0x2545_f491_4f6c_dd1d;
    }
    let mut out = String::with_capacity(cfg.password_length);
    for _ in 0..cfg.password_length {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let next = x.wrapping_mul(0x2545_f491_4f6c_dd1d);
        out.push(chars[(next >> 59) as usize % chars.len()] as char);
    }
    out
}

/// A password is storable when the line encoding can hold it.
fn password_ok(password: &str) -> bool {
    !password.is_empty() && password.len() <= 128 && !password.contains(['|', '\n', '\r'])
}

/// What a manual command does: the command to send now, if any, and any
/// chat lines for the player.
#[derive(Default)]
pub struct CommandOut {
    pub send: Option<String>,
    pub lines: Vec<String>,
}

const HELP: &[&str] = &[
    "/autologin - log in now with the saved password",
    "/autoregister - register now with a new generated password",
    "/autologin allow - allow auto-login and auto-register on this server",
    "/autologin set <password> - save a password for this server (type it twice)",
    "/autologin pattern login <text> - what counts as a login prompt",
    "/autologin pattern register <text> - what counts as a register prompt",
    "/autologin delay <seconds> - wait before auto-login/register (0-30)",
    "/autologin toggle login|register - auto-detect on/off",
    "/autologin status - this server, saved password, consent, patterns",
    "/al - short for /autologin",
];

/// A command the player typed, without its slash. None when it is not one
/// of this mod's commands. `key` is the server address and player identity.
pub fn command(key: Option<(&str, &str)>, line: &str) -> Option<CommandOut> {
    let line = line.strip_prefix('/').unwrap_or(line);
    let mut words = line.split_whitespace();
    let first = words.next()?;
    // /al is a short alias for /autologin.
    let first = if first == "al" { "autologin" } else { first };
    if first != "autologin" && first != "autoregister" {
        return None;
    }
    let mut out = CommandOut::default();
    let mut s = state();
    if first == "autoregister" {
        let Some((server, player)) = key else {
            out.lines.push("[AutoLogin] Not on a server.".to_string());
            return Some(out);
        };
        let password = generate_password(&s.config);
        let persist = s.config.autosave;
        s.set_password(server, player, &password, persist);
        s.grant_consent(server, player);
        s.pending = None;
        s.attempted = true;
        out.send = Some(render_template(&s.config.register_template, &password));
        out.lines.push(format!(
            "[AutoLogin] Registered on {server} with a new password: {password}{}",
            if persist {
                " (saved)"
            } else {
                " (not saved: autosave is off)"
            },
        ));
        return Some(out);
    }
    // /autologin ...
    let args: Vec<&str> = words.collect();
    match args.as_slice() {
        [] => {
            let Some((server, player)) = key else {
                out.lines.push("[AutoLogin] Not on a server.".to_string());
                return Some(out);
            };
            let saved = s.password(server, player).map(|p| p.to_string());
            match saved {
                Some(pw) => {
                    s.pending = None;
                    s.attempted = true;
                    s.grant_consent(server, player);
                    out.send = Some(render_template(&s.config.login_template, &pw));
                    out.lines
                        .push(format!("[AutoLogin] Logging in on {server}..."));
                }
                None => out.lines.push(format!(
                    "[AutoLogin] No saved password for {server} - use /autologin set <password>."
                )),
            }
        }
        ["help"] => out.lines.extend(HELP.iter().map(|l| l.to_string())),
        ["allow"] => {
            let Some((server, player)) = key else {
                out.lines.push("[AutoLogin] Not on a server.".to_string());
                return Some(out);
            };
            if s.has_consent(server, player) {
                out.lines.push(format!(
                    "[AutoLogin] Auto-login is already allowed on {server}."
                ));
            } else {
                s.grant_consent(server, player);
                out.lines.push(format!(
                    "[AutoLogin] Allowed on {server}: it will log in and register automatically from now on."
                ));
            }
        }
        ["status"] => {
            let server = key.map(|(s, _)| s).unwrap_or("<none>");
            let saved = key.is_some_and(|(srv, ply)| s.password(srv, ply).is_some());
            out.lines.push(format!("[AutoLogin] Server: {server}"));
            out.lines.push(format!(
                "[AutoLogin] Saved password: {}",
                if saved { "yes" } else { "no" }
            ));
            let consented = key.is_some_and(|(srv, ply)| s.has_consent(srv, ply));
            out.lines.push(format!(
                "[AutoLogin] Consent: {}",
                if consented { "yes" } else { "no" }
            ));
            out.lines.push(format!(
                "[AutoLogin] Patterns: login \"{}\", register \"{}\"",
                s.config.login_pattern, s.config.register_pattern
            ));
            out.lines.push(format!(
                "[AutoLogin] Auto-detect: login {}, register {}; delay {}s",
                on_off(s.config.autodetect_login),
                on_off(s.config.autodetect_register),
                s.config.delay_seconds,
            ));
        }
        ["set", rest @ ..] => {
            let Some((server, player)) = key else {
                out.lines.push("[AutoLogin] Not on a server.".to_string());
                return Some(out);
            };
            let password = rest.join(" ");
            // The password is typed twice, like a register page: the
            // first line stages it and shows the consent notice, the
            // second line saves it only when it matches. A staged
            // password lives in memory only and belongs to this server.
            if let Some((srv, ply, staged)) = s.staged.clone() {
                if srv == server && ply == player {
                    if staged == password {
                        s.staged = None;
                        s.set_password(server, player, &password, true);
                        s.grant_consent(server, player);
                        out.lines
                            .push(format!("[AutoLogin] Password saved for {server}."));
                    } else {
                        s.staged = None;
                        out.lines.push(
                            "[AutoLogin] The passwords don't match - nothing saved. Start again with /autologin set <password>."
                                .to_string(),
                        );
                    }
                    return Some(out);
                }
            }
            if !password_ok(&password) {
                out.lines.push(
                    "[AutoLogin] That password cannot be saved: use 1-128 characters, no | or line breaks."
                        .to_string(),
                );
            } else {
                s.staged = Some((server.to_string(), player.to_string(), password));
                out.lines.push(format!(
                    "[AutoLogin] Save this password for {server}? Type /autologin set <password> again to confirm."
                ));
            }
        }
        ["pattern", which, rest @ ..] if *which == "login" || *which == "register" => {
            let text = rest.join(" ");
            if text.is_empty() {
                out.lines.push(
                    "[AutoLogin] Give the prompt text, e.g. /autologin pattern login please login"
                        .to_string(),
                );
            } else if *which == "login" {
                s.config.login_pattern = text.clone();
                s.queue_config();
                out.lines
                    .push(format!("[AutoLogin] Login pattern: \"{text}\""));
            } else {
                s.config.register_pattern = text.clone();
                s.queue_config();
                out.lines
                    .push(format!("[AutoLogin] Register pattern: \"{text}\""));
            }
        }
        ["delay", n] => match n.parse::<u64>() {
            Ok(n) if n <= 30 => {
                s.config.delay_seconds = n;
                s.queue_config();
                out.lines.push(format!("[AutoLogin] Login delay: {n}s"));
            }
            _ => out
                .lines
                .push("[AutoLogin] Delay is 0-30 seconds.".to_string()),
        },
        ["toggle", which] if *which == "login" || *which == "register" => {
            if *which == "login" {
                s.config.autodetect_login = !s.config.autodetect_login;
                out.lines.push(format!(
                    "[AutoLogin] Auto-login: {}",
                    on_off(s.config.autodetect_login)
                ));
            } else {
                s.config.autodetect_register = !s.config.autodetect_register;
                out.lines.push(format!(
                    "[AutoLogin] Auto-register: {}",
                    on_off(s.config.autodetect_register)
                ));
            }
            s.queue_config();
        }
        _ => {
            out.lines
                .push("[AutoLogin] Unknown - /autologin help lists the commands.".to_string());
        }
    }
    Some(out)
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

#[cfg(test)]
fn reset() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = State::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: Option<(&str, &str)> = Some(("play.example.net:25565", "Player"));

    fn setup() {
        reset();
        load(&|_| None);
    }

    fn save(pw: &str) {
        // /autologin set takes two lines now: stage, then confirm.
        command(KEY, &format!("autologin set {pw}")).unwrap();
        command(KEY, &format!("autologin set {pw}")).unwrap();
    }

    fn setup_consented() {
        setup();
        load(&|k| {
            if k == KEY_CONSENTS {
                Some("play.example.net:25565|Player".to_string())
            } else {
                None
            }
        });
    }

    fn run_ticks(n: usize) -> Vec<String> {
        run_ticks_out(n).0
    }

    fn run_ticks_out(n: usize) -> (Vec<String>, Vec<String>) {
        let mut sends = Vec::new();
        let mut lines = Vec::new();
        for _ in 0..n {
            let out = tick(KEY);
            if let Some(send) = out.send {
                sends.push(send);
            }
            lines.extend(out.lines);
        }
        (sends, lines)
    }

    #[test]
    fn auto_login_with_saved_password() {
        setup();
        command(KEY, "autologin set hunter2").unwrap();
        let out = command(KEY, "autologin set hunter2").unwrap();
        assert!(out.lines.iter().any(|l| l.contains("saved")));
        note_chat(
            "play.example.net:25565",
            "Player",
            "Please /login to continue!",
        );
        let sends = run_ticks(25);
        assert_eq!(sends, vec!["login hunter2".to_string()]);
    }

    #[test]
    fn login_on_join_with_saved_password_and_no_prompt() {
        setup();
        save("hunter2");
        // No prompt noted at all: the saved password still logs in.
        assert_eq!(run_ticks(25), vec!["login hunter2".to_string()]);
    }

    #[test]
    fn no_login_on_join_when_autodetect_login_off() {
        setup();
        load(&|k| match k {
            "autodetect_login" => Some("false".to_string()),
            _ => None,
        });
        save("hunter2");
        // The auto-detect toggle is the off switch, even with a password saved.
        assert!(run_ticks(25).is_empty());
    }

    #[test]
    fn register_still_needs_a_prompt() {
        setup();
        // No password saved and no prompt: nothing happens.
        assert!(run_ticks(25).is_empty());
    }

    #[test]
    fn join_login_once_per_join() {
        setup();
        save("hunter2");
        assert_eq!(run_ticks(25), vec!["login hunter2".to_string()]);
        // Same join: it does not send again.
        assert!(run_ticks(25).is_empty());
        // Leave and rejoin: it sends again.
        assert!(tick(None).send.is_none());
        assert_eq!(run_ticks(25), vec!["login hunter2".to_string()]);
    }

    #[test]
    fn join_login_does_not_repeat_when_once_per_join_off() {
        setup();
        load(&|k| match k {
            "disable_once_logged_in" => Some("false".to_string()),
            _ => None,
        });
        save("hunter2");
        // Even with the once-per-join stop off, the promptless login
        // fires once per connection.
        assert_eq!(run_ticks(100), vec!["login hunter2".to_string()]);
        // A rejoin fires it again.
        assert!(tick(None).send.is_none());
        assert_eq!(run_ticks(100), vec!["login hunter2".to_string()]);
    }

    #[test]
    fn toggle_off_cancels_queued_login() {
        setup();
        save("hunter2");
        assert!(run_ticks(1).is_empty()); // login scheduled, due in 20 ticks
        command(KEY, "autologin toggle login").unwrap();
        // The queued login never goes out.
        assert!(run_ticks(30).is_empty());
        // Toggling back on re-arms it on this connection.
        command(KEY, "autologin toggle login").unwrap();
        assert_eq!(run_ticks(30), vec!["login hunter2".to_string()]);
    }

    #[test]
    fn dispatch_uses_current_password() {
        setup();
        save("hunter2");
        assert!(run_ticks(1).is_empty()); // login scheduled, due in 20 ticks
        // The saved password is replaced before the delay runs out.
        save("newpass");
        assert_eq!(run_ticks(30), vec!["login newpass".to_string()]);
    }

    #[test]
    fn dispatch_drops_login_when_password_is_gone() {
        setup();
        save("hunter2");
        assert!(run_ticks(1).is_empty()); // login scheduled, due in 20 ticks
        // The saved passwords are wiped before the delay runs out.
        load(&|_| None);
        assert!(run_ticks(30).is_empty());
    }

    #[test]
    fn register_dispatch_drops_stale_password() {
        setup_consented();
        note_chat(
            "play.example.net:25565",
            "Player",
            "Please /register to play",
        );
        assert!(run_ticks(1).is_empty()); // register scheduled, due in 20 ticks
        // The saved password is replaced before the delay runs out.
        save("myownpassword");
        // No /register goes out; the login that follows uses the
        // replaced password instead.
        assert!(run_ticks(30).is_empty());
        assert_eq!(run_ticks(15), vec!["login myownpassword".to_string()]);
    }

    #[test]
    fn migrate_password_moves_name_key_to_uuid_key() {
        setup();
        save("hunter2");
        let uuid = "01234567-89ab-cdef-0123-456789abcdef";
        assert!(migrate_password("play.example.net:25565", "Player", uuid));
        assert_eq!(
            state().password("play.example.net:25565", uuid),
            Some("hunter2")
        );
        assert!(
            state()
                .password("play.example.net:25565", "Player")
                .is_none()
        );
        // A second move is a no-op.
        assert!(!migrate_password("play.example.net:25565", "Player", uuid));
        // An existing UUID-keyed password is never overwritten.
        setup();
        save("hunter2");
        command(
            Some(("play.example.net:25565", uuid)),
            "autologin set other",
        )
        .unwrap();
        command(
            Some(("play.example.net:25565", uuid)),
            "autologin set other",
        )
        .unwrap();
        assert!(!migrate_password("play.example.net:25565", "Player", uuid));
        assert_eq!(
            state().password("play.example.net:25565", uuid),
            Some("other")
        );
        assert_eq!(
            state().password("play.example.net:25565", "Player"),
            Some("hunter2")
        );
    }

    #[test]
    fn delay_is_respected() {
        setup();
        save("hunter2");
        note_chat("play.example.net:25565", "Player", "please login");
        // 1s delay = 20 ticks; the prompt is scheduled on tick 1, due on 21.
        assert!(run_ticks(20).is_empty());
        assert_eq!(run_ticks(1), vec!["login hunter2".to_string()]);
    }

    #[test]
    fn auto_register_when_no_password() {
        setup_consented();
        note_chat(
            "play.example.net:25565",
            "Player",
            "Please /register to play",
        );
        let sends = run_ticks(25);
        assert_eq!(sends.len(), 1);
        let parts: Vec<&str> = sends[0].split_whitespace().collect();
        assert_eq!(parts[0], "register");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1], parts[2]);
        assert_eq!(parts[1].len(), 6);
        assert!(
            parts[1]
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
        // The generated password is saved for the next login.
        assert!(
            state()
                .password("play.example.net:25565", "Player")
                .is_some()
        );
    }

    #[test]
    fn login_beats_register_when_password_saved() {
        setup();
        save("hunter2");
        note_chat(
            "play.example.net:25565",
            "Player",
            "please /login or /register",
        );
        let sends = run_ticks(25);
        assert_eq!(sends, vec!["login hunter2".to_string()]);
    }

    #[test]
    fn once_per_join_then_again_after_rejoin() {
        setup();
        save("hunter2");
        note_chat("play.example.net:25565", "Player", "please login");
        assert_eq!(run_ticks(25), vec!["login hunter2".to_string()]);
        // Same join: a new prompt does not send again.
        note_chat("play.example.net:25565", "Player", "please login");
        assert!(run_ticks(25).is_empty());
        // Leave and rejoin: it sends again.
        assert!(tick(None).send.is_none());
        note_chat("play.example.net:25565", "Player", "please login");
        assert_eq!(run_ticks(25), vec!["login hunter2".to_string()]);
    }

    #[test]
    fn prompt_from_another_server_is_ignored() {
        setup();
        // No password saved: only a prompt for this server could register.
        note_chat("other.example.net", "Player", "please /register to play");
        assert!(run_ticks(25).is_empty());
    }

    #[test]
    fn pattern_matching_is_case_insensitive() {
        setup();
        save("hunter2");
        note_chat("play.example.net:25565", "Player", "PLEASE LOGIN NOW");
        assert_eq!(run_ticks(25), vec!["login hunter2".to_string()]);
    }

    #[test]
    fn templates_render_and_drop_the_slash() {
        assert_eq!(render_template("/login $p", "pw"), "login pw");
        assert_eq!(render_template("/register $p $p", "pw"), "register pw pw");
        assert_eq!(render_template("login $p", "pw"), "login pw");
    }

    #[test]
    fn generated_passwords_use_the_configured_charset() {
        setup();
        let cfg = Config::default();
        for _ in 0..20 {
            let pw = generate_password(&cfg);
            assert_eq!(pw.len(), 6);
            assert!(pw.chars().all(|c| cfg.password_chars.contains(c)));
        }
    }

    #[test]
    fn manual_commands() {
        setup();
        assert!(command(KEY, "hello").is_none());
        assert!(command(KEY, "/autologin help").unwrap().lines.len() > 5);
        // No saved password: manual login says so and sends nothing.
        let out = command(KEY, "autologin").unwrap();
        assert!(out.send.is_none());
        assert!(out.lines.iter().any(|l| l.contains("No saved password")));
        // Manual register sends at once, no delay.
        let out = command(KEY, "autoregister").unwrap();
        assert!(out.send.as_ref().unwrap().starts_with("register "));
        assert!(out.lines.iter().any(|l| l.contains("new password")));
    }

    #[test]
    fn set_rejects_unstorable_passwords() {
        setup();
        assert!(
            command(KEY, "autologin set a|b")
                .unwrap()
                .lines
                .iter()
                .any(|l| l.contains("cannot be saved"))
        );
        assert!(
            command(KEY, "autologin set")
                .unwrap()
                .lines
                .iter()
                .any(|l| l.contains("cannot be saved"))
        );
        assert!(!password_ok(""));
        assert!(!password_ok("a|b"));
        assert!(!password_ok("a\nb"));
        assert!(password_ok("hunter2"));
    }

    #[test]
    fn patterns_and_delay_change_without_rebuilding() {
        setup_consented();
        // The login pattern no longer gates login when a password is saved,
        // so the pattern is exercised through the register prompt instead.
        command(KEY, "autologin pattern register type /register now").unwrap();
        assert_eq!(state().config.register_pattern, "type /register now");
        command(KEY, "autologin delay 0").unwrap();
        assert_eq!(state().config.delay_seconds, 0);
        note_chat("play.example.net:25565", "Player", "type /register now");
        let sends = run_ticks(1);
        assert_eq!(sends.len(), 1);
        assert!(sends[0].starts_with("register "));
        // The old pattern no longer matches.
        setup();
        command(KEY, "autologin pattern register type /register now").unwrap();
        note_chat("play.example.net:25565", "Player", "please register");
        assert!(run_ticks(5).is_empty());
    }

    #[test]
    fn passwords_survive_a_store_and_load_round_trip() {
        setup();
        save("hunter2");
        let stored = take_store()
            .into_iter()
            .find(|(k, _)| k == KEY_PASSWORDS)
            .map(|(_, v)| v)
            .unwrap();
        assert_eq!(stored, "play.example.net:25565|Player|hunter2");
        reset();
        load(&|k| {
            if k == KEY_PASSWORDS {
                Some(stored.clone())
            } else {
                None
            }
        });
        assert_eq!(
            state().password("play.example.net:25565", "Player"),
            Some("hunter2")
        );
    }

    #[test]
    fn settings_load_from_strings() {
        // The new API has no button page: settings arrive as strings from
        // the host (register_setting kinds) and load through the same path.
        setup();
        load(&|k| match k {
            "autodetect_login" => Some("false".to_string()),
            "delay_seconds" => Some("5".to_string()),
            "login_pattern" => Some("type /login now".to_string()),
            _ => None,
        });
        assert!(!state().config.autodetect_login);
        assert!(state().config.autodetect_register);
        assert_eq!(state().config.delay_seconds, 5);
        assert_eq!(state().config.login_pattern, "type /login now");
        // A changed setting is honoured: with auto-login off, a login
        // prompt does nothing.
        save("hunter2");
        note_chat("play.example.net:25565", "Player", "type /login now");
        assert!(run_ticks(25).is_empty());
    }

    #[test]
    fn status_and_toggle() {
        setup();
        let out = command(KEY, "autologin status").unwrap();
        assert!(
            out.lines
                .iter()
                .any(|l| l.contains("play.example.net:25565"))
        );
        assert!(out.lines.iter().any(|l| l.contains("Saved password: no")));
        command(KEY, "autologin toggle login").unwrap();
        assert!(!state().config.autodetect_login);
        // With auto-login off, a login prompt does nothing.
        save("hunter2");
        note_chat("play.example.net:25565", "Player", "please login");
        assert!(run_ticks(25).is_empty());
    }

    #[test]
    fn manual_login_cancels_a_pending_auto_login() {
        setup();
        save("hunter2");
        note_chat("play.example.net:25565", "Player", "please login");
        assert!(run_ticks(5).is_empty()); // still in the delay
        let out = command(KEY, "autologin").unwrap();
        assert_eq!(out.send.as_deref(), Some("login hunter2"));
        // The pending auto-login does not fire afterwards.
        assert!(run_ticks(25).is_empty());
    }

    #[test]
    fn join_login_waits_for_consent() {
        setup();
        // A password saved without a consent record: the mod asks once
        // and sends nothing.
        load(&|k| {
            if k == KEY_PASSWORDS {
                Some("play.example.net:25565|Player|hunter2".to_string())
            } else {
                None
            }
        });
        let (sends, lines) = run_ticks_out(25);
        assert!(sends.is_empty());
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.contains("Allow auto-login"))
                .count(),
            1
        );
        // The notice does not repeat on the same connection.
        let (_, lines) = run_ticks_out(25);
        assert!(lines.iter().all(|l| !l.contains("Allow auto-login")));
        // Confirming grants consent; the login fires on the next ticks.
        command(KEY, "autologin allow").unwrap();
        assert_eq!(run_ticks(25), vec!["login hunter2".to_string()]);
    }

    #[test]
    fn register_asks_before_registering() {
        setup();
        note_chat(
            "play.example.net:25565",
            "Player",
            "Please /register to play",
        );
        let (sends, lines) = run_ticks_out(5);
        assert!(sends.is_empty());
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.contains("Allow Auto Login to register"))
                .count(),
            1
        );
        // Confirming schedules the register; the generated password is
        // saved only when it is actually sent.
        command(KEY, "autologin allow").unwrap();
        let sends = run_ticks(25);
        assert_eq!(sends.len(), 1);
        assert!(sends[0].starts_with("register "));
        assert!(
            state()
                .password("play.example.net:25565", "Player")
                .is_some()
        );
    }

    #[test]
    fn set_needs_two_matching_lines() {
        setup();
        let out = command(KEY, "autologin set hunter2").unwrap();
        assert!(out.lines.iter().any(|l| l.contains("again to confirm")));
        // Nothing is saved after the first line.
        assert!(
            state()
                .password("play.example.net:25565", "Player")
                .is_none()
        );
        // A different second line discards the staged password.
        let out = command(KEY, "autologin set hunter3").unwrap();
        assert!(out.lines.iter().any(|l| l.contains("don't match")));
        assert!(
            state()
                .password("play.example.net:25565", "Player")
                .is_none()
        );
        // Two matching lines save it, and record consent.
        save("hunter2");
        assert_eq!(
            state().password("play.example.net:25565", "Player"),
            Some("hunter2")
        );
        assert!(state().has_consent("play.example.net:25565", "Player"));
    }

    #[test]
    fn pending_register_password_discarded_on_cancel() {
        setup_consented();
        note_chat(
            "play.example.net:25565",
            "Player",
            "Please /register to play",
        );
        assert!(run_ticks(1).is_empty()); // register scheduled, due in 20 ticks
        // The generated password is not saved yet.
        assert!(
            state()
                .password("play.example.net:25565", "Player")
                .is_none()
        );
        // Cancelling before dispatch leaves no credential behind.
        command(KEY, "autologin toggle register").unwrap();
        assert!(run_ticks(30).is_empty());
        assert!(
            state()
                .password("play.example.net:25565", "Player")
                .is_none()
        );
        // Toggling back on re-arms it from the same prompt.
        command(KEY, "autologin toggle register").unwrap();
        let sends = run_ticks(30);
        assert_eq!(sends.len(), 1);
        assert!(sends[0].starts_with("register "));
        assert!(
            state()
                .password("play.example.net:25565", "Player")
                .is_some()
        );
    }

    #[test]
    fn proxy_reconfiguration_keeps_one_attempt() {
        setup();
        save("hunter2");
        assert_eq!(run_ticks(25), vec!["login hunter2".to_string()]);
        // A new login on a different address, with no off-server tick
        // between (a proxy reconfiguration): the limit survives, so
        // nothing is sent again.
        new_connection();
        let other = Some(("other.example.net:25565", "Player"));
        assert!(tick(other).send.is_none());
        let mut sends = Vec::new();
        for _ in 0..25 {
            if let Some(send) = tick(other).send {
                sends.push(send);
            }
        }
        assert!(sends.is_empty());
        // Leaving the server and rejoining resets the limit.
        assert!(tick(None).send.is_none());
        assert_eq!(run_ticks(25), vec!["login hunter2".to_string()]);
    }

    #[test]
    fn manual_login_grants_consent() {
        setup();
        load(&|k| {
            if k == KEY_PASSWORDS {
                Some("play.example.net:25565|Player|hunter2".to_string())
            } else {
                None
            }
        });
        let out = command(KEY, "autologin").unwrap();
        assert_eq!(out.send.as_deref(), Some("login hunter2"));
        assert!(state().has_consent("play.example.net:25565", "Player"));
    }

    #[test]
    fn al_alias_works() {
        setup();
        assert!(
            command(KEY, "al status")
                .unwrap()
                .lines
                .iter()
                .any(|l| l.contains("Server:"))
        );
        assert!(command(KEY, "al help").unwrap().lines.len() > 5);
        command(KEY, "al set hunter2").unwrap();
        command(KEY, "al set hunter2").unwrap();
        assert_eq!(
            state().password("play.example.net:25565", "Player"),
            Some("hunter2")
        );
        let out = command(KEY, "al").unwrap();
        assert_eq!(out.send.as_deref(), Some("login hunter2"));
    }

    #[test]
    fn staged_password_cleared_on_key_change() {
        setup();
        command(KEY, "autologin set hunter2").unwrap();
        // Switching server clears the staged password.
        let other = Some(("other.example.net:25565", "Player"));
        assert!(tick(other).send.is_none());
        assert!(tick(KEY).send.is_none());
        // The same second line is a fresh first line again: nothing saved.
        let out = command(KEY, "autologin set hunter2").unwrap();
        assert!(out.lines.iter().any(|l| l.contains("again to confirm")));
        assert!(
            state()
                .password("play.example.net:25565", "Player")
                .is_none()
        );
        command(KEY, "autologin set hunter2").unwrap();
        assert_eq!(
            state().password("play.example.net:25565", "Player"),
            Some("hunter2")
        );
    }
}
