//! Auto Login & Auto Register for Horizon (native .hmod): the hooks that
//! feed the pure-logic engine in `engine.rs`.
//!
//! The engine keeps all state behind one mutex; the hooks do the game's I/O:
//! - `client_chat_received` notes chat lines for pattern matching — overlay
//!   (action-bar) lines included, since some servers prompt there instead
//!   of chat. Title text has no hook in the mod API, so a title-only
//!   prompt is out of this build's reach (the .hnmod reads those packets
//!   directly and covers it).
//! - `customize_boss_event_progress` notes the boss bar's name for the same
//!   matching, since some servers prompt there instead of chat. The hook
//!   fires once a frame per bar, so each bar is tracked separately and
//!   only a new or changed bar is noted; the set is cleared on every
//!   new connection.
//! - `register_client_commands` declares `/autologin` and `/autoregister`
//!   (with their subcommands) to the chat tree, so they colour and
//!   complete instead of showing red while typing. `/al` is a short alias
//!   for `/autologin`, and `/autologin allow` records consent for the
//!   server the player is on.
//! - `client_tick_pre` runs the delay and sends the login/register command;
//!   on join it logs in at once when a password is saved for the server
//!   and consent is recorded for it, with no prompt needed — prompts only
//!   matter when nothing is saved. The promptless login fires once per
//!   connection, a queued send is cancelled when its toggle goes off, and
//!   the toggle and the saved password are rechecked immediately before
//!   dispatch. A generated register password is saved only when the
//!   `/register` is actually sent. The one-attempt limit survives a proxy
//!   reconfiguration: it resets only on a real off-server tick, not when
//!   the server address merely changes.
//! - `command` answers the mod's own `/autologin` and `/autoregister`,
//! - `register` declares the four settings on the mod's page in Options > Mods,
//! - `client_player_logging_in` resets per connection and remembers the
//!   local player's UUID, so the profile name can be read from the tab list.
//!   It also clears the boss bars seen on the previous connection. It does
//!   not reset the one-attempt limit — that survives a proxy
//!   reconfiguration and is reset only by a real off-server tick.
//!
//! The four registered settings hold the values the settings page edits.
//! Everything else the engine stores lives in the mod's data files
//! (`horizon-mod-data/autologin/`): `config` holds `key=value` lines for the
//! remaining config (patterns, templates, password options), `passwords`
//! holds `server|player|password` lines, keyed by server and the account's
//! UUID — a saved password is only ever sent for the exact server and
//! account it was saved for — and `consents` holds `server|player` lines,
//! one per server the player allowed auto-login and auto-register on. A
//! saved password without a consent record stays inactive until the player
//! allows it. Older name-keyed entries migrate to the UUID key on join.

use mod_api::{
    self, ClientChatReceivedEvent, ClientHost, ClientPlayerLoggingInEvent, ClientTickPreEvent,
    Command, CommandEvent, ConfigPath, CustomizeBossEventProgressEvent, Flow, Hooks,
    RegisterClientCommandsEvent, Registrar, RegistrationContext, SettingChangedEvent, SettingSpec,
    Text,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

mod engine;

/// The settings the mod's page edits; the rest of the config lives in the
/// `config` data file, so the commands can still change it.
const REGISTERED_SETTINGS: [&str; 4] = [
    "autodetect_login",
    "autodetect_register",
    "delay_seconds",
    "show_debug",
];

const CONFIG_FILE: &str = "config";
const PASSWORDS_FILE: &str = "passwords";
const CONSENTS_FILE: &str = "consents";

#[derive(Default)]
struct Mod {
    /// (local player's UUID from `client_player_logging_in`, resolved profile
    /// name once the tab list shows it).
    identity: Mutex<(Option<u128>, Option<String>)>,
    /// Plain name last noted per boss-bar UUID, so the per-frame
    /// boss-bar hook only wakes the engine when a bar is new or its
    /// text changed. One remembered bar is not enough: with two bars
    /// on screen they would take turns being "not the last one" and
    /// both would be re-processed every frame.
    seen_boss_bars: Mutex<HashMap<u128, String>>,
}

/// One data file as text, if it exists and is UTF-8.
fn read_data(host: &dyn ClientHost, name: &str) -> Option<String> {
    let path = ConfigPath::new(name).ok()?;
    let bytes = host.data_files().read(&path).ok()?;
    String::from_utf8(bytes).ok()
}

/// Replace one data file with `text`.
fn write_data(host: &dyn ClientHost, name: &str, text: &str) {
    if let Ok(path) = ConfigPath::new(name) {
        let _ = host.data_files().write(&path, text.as_bytes());
    }
}

/// `key=value` lines, one per line; the first `=` splits key from value.
fn parse_config(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

/// Read the settings and data files into the engine. The registered settings
/// come from the host's typed reads; the rest of the config and the saved
/// passwords come from the data files.
fn load(host: &dyn ClientHost) {
    let config = read_data(host, CONFIG_FILE)
        .map(|text| parse_config(&text))
        .unwrap_or_default();
    let passwords = read_data(host, PASSWORDS_FILE);
    let consents = read_data(host, CONSENTS_FILE);
    engine::load(&|key| match key {
        "autodetect_login" => host.setting_bool(key).map(|b| b.to_string()),
        "autodetect_register" => host.setting_bool(key).map(|b| b.to_string()),
        "show_debug" => host.setting_bool(key).map(|b| b.to_string()),
        "delay_seconds" => host.setting_f64(key).map(|f| format!("{}", f as u64)),
        "passwords" => passwords.clone(),
        "consents" => consents.clone(),
        _ => config.get(key).cloned(),
    });
}

/// Write the engine's queued store writes: registered settings through the
/// host, passwords and the rest of the config through data files.
fn persist(host: &dyn ClientHost) {
    let store = engine::take_store();
    if store.is_empty() {
        return;
    }
    let mut config: BTreeMap<String, String> = read_data(host, CONFIG_FILE)
        .map(|text| parse_config(&text))
        .unwrap_or_default();
    let mut config_changed = false;
    for (key, value) in store {
        if key == PASSWORDS_FILE {
            write_data(host, PASSWORDS_FILE, &value);
        } else if key == CONSENTS_FILE {
            write_data(host, CONSENTS_FILE, &value);
        } else if REGISTERED_SETTINGS.contains(&key.as_str()) {
            host.set_setting(engine::ID, &key, &value);
        } else {
            config.insert(key, value);
            config_changed = true;
        }
    }
    if config_changed {
        let text = config
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\n");
        write_data(host, CONFIG_FILE, &text);
    }
}

/// A UUID as the hyphenated lowercase string the password file keys on.
fn uuid_string(uuid: u128) -> String {
    let hex = format!("{uuid:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The (server, player) key the engine files this connection under. The
/// player part is the local account's UUID, so a saved password can only
/// be looked up for the exact server and account it was saved for — a
/// tab-list name is only a display label, and labels can be shared
/// across connections. Until the UUID is known the profile name keys it.
fn current_key(
    host: &dyn ClientHost,
    identity: &Mutex<(Option<u128>, Option<String>)>,
) -> Option<(String, String)> {
    let server = host.client_session().client_server()?.address?;
    let uuid = {
        let id = identity.lock().unwrap_or_else(|e| e.into_inner());
        id.0
    };
    if let Some(uuid) = uuid {
        return Some((server, uuid_string(uuid)));
    }
    let player = local_player_name(host, identity).unwrap_or_default();
    Some((server, player))
}

/// The local player's profile name, from the tab list entry with the UUID
/// `client_player_logging_in` reported; cached once found.
fn local_player_name(
    host: &dyn ClientHost,
    identity: &Mutex<(Option<u128>, Option<String>)>,
) -> Option<String> {
    let mut id = identity.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(name) = &id.1 {
        return Some(name.clone());
    }
    let uuid = id.0?;
    let list = host.client_session().client_tab_list()?;
    let entry = list.entries.iter().find(|e| e.uuid == uuid)?;
    let name = entry.name.clone();
    id.1 = Some(name.clone());
    Some(name)
}

/// Send the engine's answer: the command (slash first) and its chat lines.
fn answer(host: &mut dyn ClientHost, send: Option<String>, lines: Vec<String>) {
    if let Some(command) = send {
        let _ = host.send_chat(&format!("/{command}"));
    }
    for line in lines {
        let _ = host.chat_line(Text::literal(line));
    }
}

#[mod_api::horizon_mod]
impl Hooks for Mod {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let _ = r.register_setting(SettingSpec::toggle("autodetect_login", "Auto-login", true));
        let _ = r.register_setting(SettingSpec::toggle(
            "autodetect_register",
            "Auto-register",
            true,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "delay_seconds",
            "Login delay",
            0.0,
            30.0,
            1.0,
            1.0,
        ));
        let _ = r.register_setting(SettingSpec::toggle("show_debug", "Debug messages", false));
    }

    fn register_client_commands(
        &self,
        _host: &mut dyn ClientHost,
        event: &mut RegisterClientCommandsEvent<'_>,
    ) {
        // The usages the engine answers, so the chat box colours, completes
        // and lists them as the game's own instead of showing red.
        event.commands.push(Command::new("autologin"));
        event.commands.push(Command::new("autologin allow"));
        event
            .commands
            .push(Command::new("autologin set <password:text>"));
        event.commands.push(Command::new("autologin status"));
        event.commands.push(Command::new("autologin help"));
        event
            .commands
            .push(Command::new("autologin toggle <which:word>"));
        event
            .commands
            .push(Command::new("autologin delay <seconds:integer>"));
        event.commands.push(Command::new(
            "autologin pattern <which:word> <pattern:text>",
        ));
        event.commands.push(Command::new("autoregister"));
        // /al is the short alias for /autologin, with the same subcommands.
        event.commands.push(Command::new("al"));
        event.commands.push(Command::new("al allow"));
        event.commands.push(Command::new("al set <password:text>"));
        event.commands.push(Command::new("al status"));
        event.commands.push(Command::new("al help"));
        event.commands.push(Command::new("al toggle <which:word>"));
        event
            .commands
            .push(Command::new("al delay <seconds:integer>"));
        event
            .commands
            .push(Command::new("al pattern <which:word> <pattern:text>"));
    }

    fn customize_boss_event_progress(
        &self,
        host: &dyn ClientHost,
        event: &mut CustomizeBossEventProgressEvent<'_>,
    ) -> Flow {
        if !host.on(engine::ID) {
            return Flow::Continue;
        }
        let Ok(text) = host.text_string(&event.bar.name) else {
            return Flow::Continue;
        };
        let Some((server, player)) = current_key(host, &self.identity) else {
            return Flow::Continue;
        };
        let seen = (event.bar.uuid, text.clone());
        {
            let mut seen_bars = self
                .seen_boss_bars
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if seen_bars.get(&seen.0) == Some(&seen.1) {
                return Flow::Continue;
            }
            seen_bars.insert(seen.0, seen.1);
        }
        load(host);
        engine::note_chat(&server, &player, &text);
        persist(host);
        Flow::Continue
    }

    fn client_chat_received(
        &self,
        host: &mut dyn ClientHost,
        event: &mut ClientChatReceivedEvent<'_>,
    ) -> Flow {
        if !host.on(engine::ID) {
            return Flow::Continue;
        }
        let Some((server, player)) = current_key(host, &self.identity) else {
            return Flow::Continue;
        };
        let Ok(text) = host.text_string(&*event.message) else {
            return Flow::Continue;
        };
        load(host);
        engine::note_chat(&server, &player, &text);
        persist(host);
        Flow::Continue
    }

    fn client_tick_pre(&self, host: &mut dyn ClientHost, _event: &mut ClientTickPreEvent) {
        if !host.on(engine::ID) {
            return;
        }
        let key = current_key(host, &self.identity);
        load(host);
        // Migrate a password — and its consent record — saved under the
        // profile name to the account-UUID key, once the tab list has
        // shown the name. A no-op after the move (or when nothing was
        // saved that way): the key is the UUID exactly when it differs
        // from the name.
        if let Some((server, player)) = &key {
            if let Some(name) = local_player_name(host, &self.identity) {
                if *player != name {
                    engine::migrate_password(server, &name, player);
                    engine::migrate_consent(server, &name, player);
                }
            }
        }
        let out = engine::tick(key.as_ref().map(|(s, p)| (s.as_str(), p.as_str())));
        answer(host, out.send, out.lines);
        persist(host);
    }

    fn command(&self, host: &mut dyn ClientHost, event: &mut CommandEvent<'_>) -> Flow {
        if !host.on(engine::ID) {
            return Flow::Continue;
        }
        let key = current_key(host, &self.identity);
        load(host);
        let Some(out) = engine::command(
            key.as_ref().map(|(s, p)| (s.as_str(), p.as_str())),
            event.command,
        ) else {
            return Flow::Continue;
        };
        answer(host, out.send, out.lines);
        persist(host);
        Flow::Cancel
    }

    fn setting_changed(&self, host: &mut dyn ClientHost, _event: &mut SettingChangedEvent<'_>) {
        if !host.on(engine::ID) {
            return;
        }
        // The new value is kept already; re-read so the engine picks it up.
        load(host);
    }

    fn client_player_logging_in(
        &self,
        host: &mut dyn ClientHost,
        event: &mut ClientPlayerLoggingInEvent,
    ) {
        {
            let mut identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
            *identity = (Some(event.uuid), None);
        }
        {
            let mut seen_bars = self
                .seen_boss_bars
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            seen_bars.clear();
        }
        if !host.on(engine::ID) {
            return;
        }
        load(host);
        // A new connection: clear the previous session's prompts and
        // queued sends. The one-attempt limit is NOT reset here — it
        // survives a proxy reconfiguration and is reset only by a real
        // off-server tick.
        engine::new_connection();
        persist(host);
    }
}
