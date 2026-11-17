# horizon-autologin

<p align="center"><img src="logo.png" alt="horizon-autologin logo: a padlock circled by two arrows" width="180"></p>

Auto Login & Auto Register as a native Horizon mod (Rust): detects
login/register prompts in chat and logs you in or registers you.

> **Releases:** grab the ready-built `.hnmod` from the
> [latest release](https://github.com/SkyeSlav/horizon-autologin/releases/latest)
> — no build needed.

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

Pinned to Horizon `ac874422ed` (Starling 0.8.0, game 26.3). Horizon has no
stable incoming-chat hook, so detection injects into the game's packet
handling (`Game::handle_packet`, the same pattern as the voice-chat mod);
a newer Horizon may need the pin moved.

## Install

Build the `.hnmod` from this repo:

```sh
python3 tools/package.py        # target/autologin-1.0.0.hnmod
```

### Starling (Android)

Add `autologin-1.0.0.hnmod` in Starling's Native mods and build with your
build server (Starling 0.8.0). The mod is source-only; the build server
compiles it into the game library.

### PC launcher

```sh
python3 tools/package.py --install   # into the launcher's native mods folder
```

Then build the game with its mods in the launcher.

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

The settings page (Options > Mods > Auto Login) has buttons for the
auto-login / auto-register toggles, the delay (0, 1, 2, 3, 5s) and debug
messages. Patterns are free text, so they are set with the commands above.

## How it works

- A prompt is a chat line containing the pattern (case-insensitive).
  Login prompts come as SystemChat from auth plugins; PlayerChat and
  DisguisedChat are watched too.
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

- `mixins/autologin/engine/`: the mod's own crate (std only), with its unit
  tests — prompts, delay, once-per-join, commands, password storage.
- `mixins/autologin/*.rs`: one small hook adapter per hook point
  (packet, tick, command, commands, list, settings).
- `mixins/autologin/horizon-mixin.toml`: where each hook goes.
- `compatibility.json`: the Horizon and horizon-mixins pins.
- `tools/package.py`: makes the `.hnmod`.

## Tested

- Engine unit tests: 16 tests (auto-login with a saved password, the 1s
  delay, auto-register with a generated password, login preferred over
  register, once per join and again after rejoin, prompts from another
  server ignored, case-insensitive matching, template rendering, the
  configured password charset, manual commands, unstorable passwords
  rejected, patterns and delay changed without rebuilding, password
  store/load round-trip, settings buttons, status/toggle, manual login
  cancelling a pending auto-login) — all passing on kara-box
  (`cargo test`, Rust 1.99.0).
- `tools/package.py`: produces `target/autologin-1.0.0.hnmod` (11 files,
  12,495 bytes, sha256
  `d94dc2d72480294903ebbd2b0eafffcb13e9b736143cd7b89e0a6233765e9c5e`).
  The same build runs in GitHub Actions on every `v*` tag and attaches the
  `.hnmod` to the release — the asset on the release page is byte-identical
  to a local rebuild from these sources.
- The `.hnmod` builds through the Starling 0.8.0 build server on kara-box:
  job `0632d26e06dd1aa7491df2c1` → `{"state": "done", "sha256":
  "dc142a8df75c2814b9c744ddc51ba41be97f93b2af8ab312c82c416ae6434ced",
  "size": 66666512, "build": {"key": "23c33cdb00225d4b", "version":
  "0.8.0", "commit": "ac874422ed8f892505ff73d67ae486ecfee7a516",
  "profile": "fast", "abi": "arm64-v8a", "mixins":
  "ddd9700b21997c95d95ac6ee5ecafe3830cf622a", "mods": [{"id":
  "autologin", "version": "1.0.0"}]}}`. The built
  `libhorizon_android.so` downloads from the server (HTTP 200,
  66,666,512 bytes, sha256 identical to the job's).
- Not tested yet: a live login on a real server from a phone. That is the
  gate — see the phone test below.

## Phone test

1. Add `autologin-1.0.0.hnmod` in Starling's Native mods and build.
2. Join a server that asks for `/login` (one you already have a password
   for — first save it with `/autologin set <password>` on a join, or let
   the mod register you).
3. Watch it log you in by itself after ~1s, without typing anything.
4. On a fresh server, watch it register with a generated password and tell
   you the password once in chat.

## License

MIT, see [LICENSE](LICENSE).
