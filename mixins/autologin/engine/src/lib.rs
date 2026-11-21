//! Auto Login & Auto Register for Horizon: detects login/register prompts in
//! incoming chat and logs you in with the saved password for that server, or
//! registers you with a generated password and saves it.
//!
//! The game thread calls [`note_chat`] for every chat packet, [`tick`] once
//! per client tick on a server, and [`command`] for the mod's own commands.
//! All state lives behind one mutex; the hook adapters in `../` do the
//! game's I/O (settings files, sending commands, chat lines).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ID: &str = "autologin";
pub const NAME: &str = "Auto Login";
pub const VERSION: &str = "1.1.3";
pub const DESCRIPTION: &str = "Detects login/register prompts in chat, in title text and in the action bar, and logs you in or registers you, per server.";

/// The delay choices the settings page cycles through, in seconds.
const DELAYS: [u64; 5] = [0, 1, 2, 3, 5];

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
            get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(default)
        };
        let text = |k: &str, default: &str| get(k).filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string());
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
            password_length: if length == 0 { 6 } else { length.min(64) as usize },
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
            (KEY_AUTODETECT_REGISTER, self.autodetect_register.to_string()),
            (KEY_USE_DELAY, self.use_delay.to_string()),
            (KEY_DELAY_SECONDS, self.delay_seconds.to_string()),
            (KEY_LOGIN_TEMPLATE, self.login_template.clone()),
            (KEY_REGISTER_TEMPLATE, self.register_template.clone()),
            (KEY_PASSWORD_LENGTH, self.password_length.to_string()),
            (KEY_PASSWORD_CHARS, self.password_chars.clone()),
            (KEY_DISABLE_ONCE_LOGGED_IN, self.disable_once_logged_in.to_string()),
            (KEY_DISABLE_ONCE_REGISTERED, self.disable_once_registered.to_string()),
            (KEY_SHOW_DEBUG, self.show_debug.to_string()),
            (KEY_AUTOSAVE, self.autosave.to_string()),
        ]
    }

    /// The settings page's buttons, in order.
    fn buttons(&self) -> Vec<String> {
        vec![
            format!("Auto-login: {}", if self.autodetect_login { "On" } else { "Off" }),
            format!("Auto-register: {}", if self.autodetect_register { "On" } else { "Off" }),
            format!("Login delay: {}s", self.delay_seconds),
            format!("Debug messages: {}", if self.show_debug { "On" } else { "Off" }),
        ]
    }
}

/// A login or register the delay is counting down for.
struct Pending {
    login: bool,
    command: String,
    due: u64,
    /// The tick of the prompt that scheduled it; prompts up to here are spent.
    stamp: u64,
}

struct State {
    config: Config,
    passwords: Vec<Password>,
    /// Settings writes the adapter still has to persist.
    store: Vec<(String, String)>,
    tick: u64,
    current: Option<(String, String)>,
    current_since: u64,
    attempted: bool,
    pending: Option<Pending>,
    /// (tick, server, player, text), oldest first, capped.
    recent: Vec<(u64, String, String, String)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            config: Config::default(),
            passwords: Vec::new(),
            store: Vec::new(),
            tick: 0,
            current: None,
            current_since: 0,
            attempted: false,
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
}

/// Settings writes the adapter still has to persist.
pub fn take_store() -> Vec<(String, String)> {
    std::mem::take(&mut state().store)
}

/// A chat line arrived from `server` for `player`. Only its text is kept.
pub fn note_chat(server: &str, player: &str, text: &str) {
    let mut s = state();
    let tick = s.tick;
    s.recent.push((tick, server.to_string(), player.to_string(), text.to_string()));
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

/// One client tick on a server. `key` is the server address and profile
/// name, or None when not on a server.
pub fn tick(key: Option<(&str, &str)>) -> TickOut {
    let mut out = TickOut::default();
    let mut s = state();
    s.tick += 1;
    let Some((server, player)) = key else {
        s.current = None;
        s.pending = None;
        s.recent.clear();
        return out;
    };
    if s.current.as_ref().map(|(a, b)| (a.as_str(), b.as_str())) != Some((server, player)) {
        s.current = Some((server.to_string(), player.to_string()));
        // Prompts for this server noted just before the first tick of the
        // join still count; prompts from another server do not.
        s.recent.retain(|r| r.1 == server && r.2 == player);
        s.current_since = s.recent.iter().map(|r| r.0).min().unwrap_or(s.tick);
        s.attempted = false;
        s.pending = None;
    }
    if s.attempted {
        return out;
    }
    // Schedule from the newest prompt for this server, if nothing is pending.
    if s.pending.is_none() {
        let cfg = s.config.clone();
        let since = s.current_since;
        let mut found: Option<(bool, u64)> = None;
        for (stamp, srv, ply, text) in s.recent.iter().rev() {
            if *stamp < since {
                break;
            }
            if srv != server || ply != player {
                continue;
            }
            let lower = text.to_lowercase();
            let login_hit = cfg.autodetect_login
                && lower.contains(&cfg.login_pattern.to_lowercase());
            let register_hit = cfg.autodetect_register
                && lower.contains(&cfg.register_pattern.to_lowercase());
            if login_hit && s.password(server, player).is_some() {
                found = Some((true, *stamp));
                break;
            }
            if register_hit && s.password(server, player).is_none() {
                found = Some((false, *stamp));
                break;
            }
        }
        if let Some((login, stamp)) = found {
            let (password, template) = if login {
                let pw = s.password(server, player).unwrap_or_default().to_string();
                (pw, cfg.login_template.clone())
            } else {
                let pw = generate_password(&cfg);
                let persist = cfg.autosave;
                s.set_password(server, player, &pw, persist);
                if cfg.show_debug && !persist {
                    out.lines.push(
                        "[AutoLogin] Generated a password but autosave is off: it will not survive a restart."
                            .to_string(),
                    );
                }
                (pw, cfg.register_template.clone())
            };
            let due = if cfg.use_delay {
                s.tick + cfg.delay_seconds.saturating_mul(20)
            } else {
                s.tick
            };
            if cfg.show_debug {
                out.lines.push(format!(
                    "[AutoLogin] Saw a {} prompt on {server}; sending in {}s.",
                    if login { "login" } else { "register" },
                    if cfg.use_delay { cfg.delay_seconds } else { 0 },
                ));
            }
            s.pending = Some(Pending {
                login,
                command: render_template(&template, &password),
                due,
                stamp,
            });
        }
    }
    // Send when the delay ran out.
    if let Some(p) = &s.pending {
        if s.tick >= p.due {
            let p = s.pending.take().unwrap();
            if p.login {
                if s.config.disable_once_logged_in {
                    s.attempted = true;
                }
            } else if s.config.disable_once_registered {
                s.attempted = true;
            }
            // The prompt that scheduled this is spent, so it cannot fire
            // again when the once-per-join stop is off.
            s.recent.retain(|r| r.0 > p.stamp);
            out.send = Some(p.command);
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
        ^ COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9e37_79b9_7f4a_7c15)
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
    !password.is_empty()
        && password.len() <= 128
        && !password.contains(['|', '\n', '\r'])
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
    "/autologin set <password> - save a password for this server",
    "/autologin pattern login <text> - what counts as a login prompt",
    "/autologin pattern register <text> - what counts as a register prompt",
    "/autologin delay <seconds> - wait before auto-login/register (0-30)",
    "/autologin toggle login|register - auto-detect on/off",
    "/autologin status - this server, saved password, patterns",
];

/// A command the player typed, without its slash. None when it is not one
/// of this mod's commands. `key` is the server address and profile name.
pub fn command(key: Option<(&str, &str)>, line: &str) -> Option<CommandOut> {
    let line = line.strip_prefix('/').unwrap_or(line);
    let mut words = line.split_whitespace();
    let first = words.next()?;
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
        s.pending = None;
        s.attempted = true;
        out.send = Some(render_template(&s.config.register_template, &password));
        out.lines.push(format!(
            "[AutoLogin] Registered on {server} with a new password: {password}{}",
            if persist { " (saved)" } else { " (not saved: autosave is off)" },
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
                    out.send = Some(render_template(&s.config.login_template, &pw));
                    out.lines.push(format!("[AutoLogin] Logging in on {server}..."));
                }
                None => out.lines.push(format!(
                    "[AutoLogin] No saved password for {server} - use /autologin set <password>."
                )),
            }
        }
        ["help"] => out.lines.extend(HELP.iter().map(|l| l.to_string())),
        ["status"] => {
            let (server, player) = key.unwrap_or(("<none>", "<none>"));
            let saved = key.is_some_and(|(srv, ply)| s.password(srv, ply).is_some());
            out.lines.push(format!("[AutoLogin] Server: {server} ({player})"));
            out.lines.push(format!(
                "[AutoLogin] Saved password: {}",
                if saved { "yes" } else { "no" }
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
            if !password_ok(&password) {
                out.lines.push(
                    "[AutoLogin] That password cannot be saved: use 1-128 characters, no | or line breaks."
                        .to_string(),
                );
            } else {
                s.set_password(server, player, &password, true);
                out.lines.push(format!("[AutoLogin] Password saved for {server} ({player})."));
            }
        }
        ["pattern", which, rest @ ..] if *which == "login" || *which == "register" => {
            let text = rest.join(" ");
            if text.is_empty() {
                out.lines.push("[AutoLogin] Give the prompt text, e.g. /autologin pattern login please login".to_string());
            } else if *which == "login" {
                s.config.login_pattern = text.clone();
                s.queue_config();
                out.lines.push(format!("[AutoLogin] Login pattern: \"{text}\""));
            } else {
                s.config.register_pattern = text.clone();
                s.queue_config();
                out.lines.push(format!("[AutoLogin] Register pattern: \"{text}\""));
            }
        }
        ["delay", n] => match n.parse::<u64>() {
            Ok(n) if n <= 30 => {
                s.config.delay_seconds = n;
                s.queue_config();
                out.lines.push(format!("[AutoLogin] Login delay: {n}s"));
            }
            _ => out.lines.push("[AutoLogin] Delay is 0-30 seconds.".to_string()),
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
            out.lines.push("[AutoLogin] Unknown - /autologin help lists the commands.".to_string());
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

    fn run_ticks(n: usize) -> Vec<String> {
        let mut sends = Vec::new();
        for _ in 0..n {
            if let Some(send) = tick(KEY).send {
                sends.push(send);
            }
        }
        sends
    }

    #[test]
    fn auto_login_with_saved_password() {
        setup();
        let out = command(KEY, "autologin set hunter2").unwrap();
        assert!(out.lines.iter().any(|l| l.contains("saved")));
        note_chat("play.example.net:25565", "Player", "Please /login to continue!");
        let sends = run_ticks(25);
        assert_eq!(sends, vec!["login hunter2".to_string()]);
    }

    #[test]
    fn delay_is_respected() {
        setup();
        command(KEY, "autologin set hunter2").unwrap();
        note_chat("play.example.net:25565", "Player", "please login");
        // 1s delay = 20 ticks; the prompt is scheduled on tick 1, due on 21.
        assert!(run_ticks(20).is_empty());
        assert_eq!(run_ticks(1), vec!["login hunter2".to_string()]);
    }

    #[test]
    fn auto_register_when_no_password() {
        setup();
        note_chat("play.example.net:25565", "Player", "Please /register to play");
        let sends = run_ticks(25);
        assert_eq!(sends.len(), 1);
        let parts: Vec<&str> = sends[0].split_whitespace().collect();
        assert_eq!(parts[0], "register");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1], parts[2]);
        assert_eq!(parts[1].len(), 6);
        assert!(parts[1].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        // The generated password is saved for the next login.
        assert!(state().password("play.example.net:25565", "Player").is_some());
    }

    #[test]
    fn login_beats_register_when_password_saved() {
        setup();
        command(KEY, "autologin set hunter2").unwrap();
        note_chat("play.example.net:25565", "Player", "please /login or /register");
        let sends = run_ticks(25);
        assert_eq!(sends, vec!["login hunter2".to_string()]);
    }

    #[test]
    fn once_per_join_then_again_after_rejoin() {
        setup();
        command(KEY, "autologin set hunter2").unwrap();
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
        command(KEY, "autologin set hunter2").unwrap();
        note_chat("other.example.net", "Player", "please login");
        assert!(run_ticks(25).is_empty());
    }

    #[test]
    fn pattern_matching_is_case_insensitive() {
        setup();
        command(KEY, "autologin set hunter2").unwrap();
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
        assert!(command(KEY, "autologin set").unwrap().lines.iter().any(|l| l.contains("cannot be saved")));
        assert!(!password_ok(""));
        assert!(!password_ok("a|b"));
        assert!(!password_ok("a\nb"));
        assert!(password_ok("hunter2"));
    }

    #[test]
    fn patterns_and_delay_change_without_rebuilding() {
        setup();
        command(KEY, "autologin pattern login type /login now").unwrap();
        assert_eq!(state().config.login_pattern, "type /login now");
        command(KEY, "autologin delay 0").unwrap();
        assert_eq!(state().config.delay_seconds, 0);
        command(KEY, "autologin set hunter2").unwrap();
        note_chat("play.example.net:25565", "Player", "type /login now");
        assert_eq!(run_ticks(1), vec!["login hunter2".to_string()]);
        // The old pattern no longer matches.
        assert!(tick(None).send.is_none());
        note_chat("play.example.net:25565", "Player", "please login");
        assert!(run_ticks(5).is_empty());
    }

    #[test]
    fn passwords_survive_a_store_and_load_round_trip() {
        setup();
        command(KEY, "autologin set hunter2").unwrap();
        let stored = take_store()
            .into_iter()
            .find(|(k, _)| k == KEY_PASSWORDS)
            .map(|(_, v)| v)
            .unwrap();
        assert_eq!(stored, "play.example.net:25565|Player|hunter2");
        reset();
        load(&|k| if k == KEY_PASSWORDS { Some(stored.clone()) } else { None });
        assert_eq!(state().password("play.example.net:25565", "Player"), Some("hunter2"));
    }

    #[test]
    fn settings_buttons_cycle() {
        setup();
        let buttons = settings_buttons();
        assert_eq!(buttons.len(), 4);
        assert!(buttons[0].contains("On"));
        settings_click(0);
        assert!(settings_buttons()[0].contains("Off"));
        settings_click(2);
        assert_eq!(state().config.delay_seconds, 2);
        settings_click(2);
        assert_eq!(state().config.delay_seconds, 3);
        settings_click(2);
        assert_eq!(state().config.delay_seconds, 5);
        settings_click(2);
        assert_eq!(state().config.delay_seconds, 0);
        settings_click(99);
    }

    #[test]
    fn status_and_toggle() {
        setup();
        let out = command(KEY, "autologin status").unwrap();
        assert!(out.lines.iter().any(|l| l.contains("play.example.net:25565")));
        assert!(out.lines.iter().any(|l| l.contains("Saved password: no")));
        command(KEY, "autologin toggle login").unwrap();
        assert!(!state().config.autodetect_login);
        // With auto-login off, a login prompt does nothing.
        command(KEY, "autologin set hunter2").unwrap();
        note_chat("play.example.net:25565", "Player", "please login");
        assert!(run_ticks(25).is_empty());
    }

    #[test]
    fn manual_login_cancels_a_pending_auto_login() {
        setup();
        command(KEY, "autologin set hunter2").unwrap();
        note_chat("play.example.net:25565", "Player", "please login");
        assert!(run_ticks(5).is_empty()); // still in the delay
        let out = command(KEY, "autologin").unwrap();
        assert_eq!(out.send.as_deref(), Some("login hunter2"));
        // The pending auto-login does not fire afterwards.
        assert!(run_ticks(25).is_empty());
    }
}

/// The settings page's buttons, in order.
pub fn settings_buttons() -> Vec<String> {
    state().config.buttons()
}

/// A click on the settings page: the button's setting moves to its next value.
pub fn settings_click(button: usize) {
    let mut s = state();
    match button {
        0 => s.config.autodetect_login = !s.config.autodetect_login,
        1 => s.config.autodetect_register = !s.config.autodetect_register,
        2 => {
            let next = DELAYS
                .iter()
                .position(|d| *d == s.config.delay_seconds)
                .map_or(0, |i| (i + 1) % DELAYS.len());
            s.config.delay_seconds = DELAYS[next];
        }
        3 => s.config.show_debug = !s.config.show_debug,
        _ => return,
    }
    s.queue_config();
}
