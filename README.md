# horizon-autologin

<p align="center"><img src="logo.png" alt="horizon-autologin logo: a padlock circled by two arrows" width="180"></p>

<p align="center">
  <a href="https://github.com/SkyeSlav/horizon-autologin/releases/latest"><img src="https://img.shields.io/github/v/release/SkyeSlav/horizon-autologin" alt="Latest release"></a>
  <a href="https://github.com/SkyeSlav/horizon-autologin/actions/workflows/release.yml"><img src="https://github.com/SkyeSlav/horizon-autologin/actions/workflows/release.yml/badge.svg" alt="Release build status"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/SkyeSlav/horizon-autologin" alt="License: MIT"></a>
</p>

Auto Login & Auto Register as a native Horizon mod (Rust): detects
login/register prompts in chat and logs you in or registers you.

> **Releases:** grab the ready-built files from the
> [latest release](https://github.com/SkyeSlav/horizon-autologin/releases/latest)
> — no build needed. `autologin.hmod` is for the Horizon launcher
> (0.2.70+); `autologin-1.0.0.hnmod` is for Starling 0.8.0.

- **Auto Login Detection** — watches incoming chat for the login prompt and
  sends `/login` with the password saved for that server and player.
- **Password Storage** — saves passwords per server + player (see
  [Security](#security)).
- **Auto Registration** — when the server asks you to register and no
  password is saved, generates a random password (default length 6, `a-z0-9`)
  and sends `/register <password> <password>`.
- **Manual Commands** — `/autologin`, `/autoregister`,
  `/autologin set <password>`.
- **Configurable Patterns** — change what counts as a login/register prompt
  without rebuilding, with `/autologin pattern ...` or the settings page.

Manual triggers are the slash commands `/autologin` and `/autoregister`
(Horizon's command hook).

## Formats

Two builds, same behaviour, different launchers:

- **1.1.0 `.hmod`** (`hmod/`): the native mod for the Horizon launcher
  (0.2.70+) and the registry. Pinned to Horizon Nightly 2026-11-18
  (`0b14f8ac`, game 26.3). Detection uses the typed
  `client_chat_received` hook; passwords live in the mod's data files
  (`horizon-mod-data/autologin/`); the settings page holds the toggles and
  the delay.
- **1.0.0 `.hnmod`** (`mixins/`): the Starling 0.8.0 build, pinned to
  Horizon `ac874422ed` (game 26.3). Horizon 0.8.0 has no stable
  incoming-chat hook, so detection injects into the game's packet handling
  (`Game::handle_packet`, the same pattern as the voice-chat mod).

## Install

### Horizon launcher (`.hmod`, 1.1.0)

Build it from this repo (needs Rust 1.99+ and the `cargo-horizon-mod`
tool; the tool fetches the pinned SDK itself):

```sh
cargo install --locked --git https://github.com/horizon-rust/horizon cargo-horizon-mod
cd hmod && cargo horizon-mod build   # target/horizon-mod/autologin.hmod
```

Then put `autologin.hmod` in the instance's `horizon-mods/` folder and add
`mod.autologin:true` to `horizon-mods.txt` in the same game folder — or run
`horizon-launcher native-build hmod --instance "<instance>" --trust-native-code`.

### Starling (`.hnmod`, 1.0.0)

Build the `.hnmod` from this repo:

```sh
python3 tools/package.py        # target/autologin-1.0.0.hnmod
```

### Starling (Android)

Add `autologin-1.0.0.hnmod` in Starling's Native mods and build with your
build server (Starling 0.8.0). The mod is source-only; the build server
compiles it into the game library.

## Commands

All commands are typed in chat, without needing a slash in front of the
name when the game completes them (`/autologin ...`).

- `/autologin` — log in now with the saved password for this server.
- `/autoregister` — register now with a new generated password (saved for
  this server; the password is shown once, in your own chat only).
- `/autologin set <password>` — save a password for this server.
- `/autologin pattern login <text>` — what counts as a login prompt
  (default `login`).
- `/autologin pattern register <text>` — what counts as a register prompt
  (default `register`).
- `/autologin delay <seconds>` — wait before auto-login/register (0–30,
  default 1).
- `/autologin toggle login|register` — auto-detect on/off.
- `/autologin status` — this server, whether a password is saved, the
  patterns and the delay.
- `/autologin help` — lists the commands.

The settings page (Options > Mods > Auto Login) has the auto-login /
auto-register toggles, the delay (0–30s slider) and debug messages.
Patterns are free text, so they are set with the commands above. (The
1.0.0 `.hnmod` page has the same toggles as buttons and cycles the delay
through 0, 1, 2, 3, 5s.)

## How it works

- A prompt is a chat line containing the pattern (case-insensitive).
  Login prompts come as SystemChat from auth plugins; PlayerChat and
  DisguisedChat are watched too. The `.hmod` reads them through the
  `client_chat_received` hook; the `.hnmod` injects into the game's packet
  handling.
- After a prompt, the mod waits the delay (default 1s), then sends
  `/login <password>` (if one is saved) or `/register <password> <password>`
  (generated, then saved). One attempt per join: after sending, it stops
  until you rejoin, unless you turn that off (the disable-once-logged-in /
  disable-once-registered settings, on by default).
  If the server's prompt words differ, set the patterns to match them.
- Sending uses the game's own command path, so commands are signed the way
  a typed command is.

## Security

Passwords are stored **in plain text on the device**, one line per
server in `server:username:password` format. Here they live in one file per the game's own per-mod storage
(`horizon-mods/.horizon-mod-data/autologin/passwords`). Anyone with access
to the device's files can read them. Do not reuse a password that matters
elsewhere; use a password made just for that server. Nothing is ever sent
anywhere except as the `/login` / `/register` commands to the server you
joined. This repo contains no passwords, and no real password should ever
be put in an issue, a log or a chat message about it.

Generated passwords come from a hand-rolled xorshift PRNG seeded from the
clock (std only, no dependencies): fine for a per-server game password,
not a cryptographic secret.

## Layout

- `hmod/`: the 1.1.0 native `.hmod` (Rust crate on the mod API):
  `src/engine.rs` is the shared logic with its unit tests,
  `src/lib.rs` the hooks, `horizon-mod.toml` the manifest and pins.
- `mixins/autologin/engine/`: the 1.0.0 mod's own crate (std only), with
  its unit tests — prompts, delay, once-per-join, commands, password
  storage. Same logic as `hmod/src/engine.rs`, adapted to the old
  settings storage.
- `mixins/autologin/*.rs`: one small hook adapter per hook point
  (packet, tick, command, commands, list, settings).
- `mixins/autologin/horizon-mixin.toml`: where each hook goes.
- `compatibility.json`: the Horizon and horizon-mixins pins.
- `tools/package.py`: makes the `.hnmod`.
- `horizon-registry.toml`: name, summary, description, authors and
  license for the registry entry.

## License

MIT, see [LICENSE](LICENSE).
