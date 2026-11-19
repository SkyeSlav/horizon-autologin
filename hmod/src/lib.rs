//! Auto Login & Auto Register for Horizon (native .hmod): the hooks that
//! feed the pure-logic engine in `engine.rs`.
//!
//! The engine keeps all state behind one mutex; the hooks do the game's I/O:
//! - `client_chat_received` notes chat lines for pattern matching — overlay
//!   (action-bar) lines included, since some servers prompt there instead
//!   of chat. Title text has no hook in the mod API, so a title-only
//!   prompt is out of this build's reach (the .hnmod reads those packets
//!   directly and covers it).
//! - `client_tick_pre` runs the delay and sends the login/register command,
//! - `command` answers the mod's own `/autologin` and `/autoregister`,
//! - `register` declares the four settings on the mod's page in Options > Mods,
//! - `client_player_logging_in` resets per connection and remembers the
//!   local player's UUID, so the profile name can be read from the tab list.
//!
//! The four registered settings hold the values the settings page edits.
//! Everything else the engine stores lives in the mod's data files
//! (`horizon-mod-data/autologin/`): `config` holds `key=value` lines for the
//! remaining config (patterns, templates, password options), and `passwords`
//! holds the `server|player|password` lines.

use mod_api::{
    self, ClientChatReceivedEvent, ClientHost, ClientPlayerLoggingInEvent, ClientTickPreEvent,
    CommandEvent, ConfigPath, Flow, Hooks, RegistrationContext, Registrar, SettingChangedEvent,
    SettingSpec, Text,
};
use std::collections::BTreeMap;
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

#[derive(Default)]
struct Mod {
    /// (local player's UUID from `client_player_logging_in`, resolved profile
    /// name once the tab list shows it).
    identity: Mutex<(Option<u128>, Option<String>)>,
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
    engine::load(&|key| match key {
        "autodetect_login" => host.setting_bool(key).map(|b| b.to_string()),
        "autodetect_register" => host.setting_bool(key).map(|b| b.to_string()),
        "show_debug" => host.setting_bool(key).map(|b| b.to_string()),
        "delay_seconds" => host.setting_f64(key).map(|f| format!("{}", f as u64)),
        "passwords" => passwords.clone(),
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

/// The (server, player) key the engine files this connection under. The
/// player name comes from the tab list once the server lists the local
/// player; until then the address alone keys it.
fn current_key(
    host: &dyn ClientHost,
    identity: &Mutex<(Option<u128>, Option<String>)>,
) -> Option<(String, String)> {
    let server = host.client_session().client_server()?.address?;
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
        if !host.on(engine::ID) {
            return;
        }
        load(host);
        // A new connection: clear the previous session's prompts and attempts.
        let _ = engine::tick(None);
        persist(host);
    }
}
