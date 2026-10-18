# Development

N-voke is a [Tauri 2](https://tauri.app) desktop app: a React + TypeScript + Tailwind UI (`src/`) on top of a small Rust core (`src-tauri/`). The UI never talks to the network or reads secrets itself. Every API request goes through the Rust `graphql` command, which holds the API key, records an audit entry for each mutation, and enforces HTTPS.

## Setup

Requirements:

- Node.js 24 or newer
- Rust (stable), installed with [rustup](https://rustup.rs)
- The [Tauri system prerequisites](https://tauri.app/start/prerequisites/) for your OS (Xcode command line tools on macOS, WebView2 and the MSVC build tools on Windows, `libwebkit2gtk-4.1-dev` and friends on Linux)

```sh
git clone https://github.com/theonlytruebigmac/n-able-scripting-demo.git
cd n-able-scripting-demo
npm install
npm run tauri dev
```

The first Rust build takes a few minutes. After that, UI edits hot-reload, and Rust edits rebuild and restart the app.

You need an N-able API-Access token to get past the first-launch screen. Create one in the N-able dashboard; the app stores it in your OS keychain.

## Everyday commands

| Command | What it does |
|---|---|
| `npm run tauri dev` | Run the full app with hot reload |
| `npm run dev` | Run only the web UI in a browser at `localhost:1420`. Calls to the Rust side fail here, so use it only for layout work |
| `npm run build` | Type-check (`tsc`) and build the frontend into `dist/` |
| `npm test` | Frontend unit tests (Vitest) |
| `cargo test --manifest-path src-tauri/Cargo.toml` | Rust unit tests |
| `npm run tauri build` | Build an installer for your current platform into `src-tauri/target/release/bundle/` |
| `npm run version:check` | Confirm every file carries the same app version |

Before you push, run the same checks CI runs:

```sh
npm run build && npm test && cargo test --manifest-path src-tauri/Cargo.toml
```

## Where things live

See the layout table in the [README](../README.md#layout). The short version:

- **Business logic is in `src/lib/`** as plain functions with unit tests next to them (`*.test.ts`): task state, tag rules, input conversion, retention, delta sync. Put new logic here, not in components, so it can be tested without a browser.
- **`src/components/`** is UI only. Pages are wired together in `src/App.tsx`.
- **`src/lib/api.ts`** holds every GraphQL query and mutation. Read the API notes in the README before changing one; several fields behave unexpectedly on just-created tasks.
- **`src-tauri/src/lib.rs`** registers the Tauri commands. Anything that touches the keychain, the filesystem or the network belongs in Rust.
- **`Invoke-NableScriptRun.ps1`** is the original PowerShell version of the same workflow. It is independent of the app.

### Adding a Tauri command

1. Write the function in `src-tauri/src/` and mark it `#[tauri::command]`.
2. Add it to the `generate_handler!` list in `lib.rs`.
3. Call it from TypeScript with `invoke` from `@tauri-apps/api/core`.

The content security policy in `src-tauri/tauri.conf.json` only allows the app's own IPC. Don't loosen `connect-src` to reach an API from the UI; add a Rust command instead.

## App icon

The master icon is `app-icon.png` in the project root (square, at least 1024px). Everything else is generated from it, so don't edit the files in `src-tauri/icons/` by hand. After replacing the master:

```sh
npx tauri icon app-icon.png
rm -rf src-tauri/icons/android src-tauri/icons/ios src-tauri/icons/64x64.png   # not used
sips -z 64 64 app-icon.png --out public/favicon.png    # browser tab icon for `npm run dev`
sips -z 128 128 app-icon.png --out src/assets/logo.png   # sidebar logo (macOS; any image tool works)
```

## Debugging

- **Log every API request:** run `NVOKE_LOG_REQUESTS=1 npm run tauri dev`. One line is printed per request, which is the easiest way to see what the cache saves.
- **Web inspector:** right-click in the app window and choose Inspect Element (dev builds only).
- **Rust logs** appear in the terminal running `npm run tauri dev`.

## Local data and resetting state

The app keeps these on your machine:

| What | Where |
|---|---|
| API key and the encryption key for local data | OS keychain, service `com.fraziersystems.nable-script-runner` (accounts `api-key` and `data-key`) |
| Settings, cache, jobs, audit log | The app data directory, named after the bundle identifier `com.fraziersystems.nable-script-runner` |

The app data directory is:

- macOS: `~/Library/Application Support/com.fraziersystems.nable-script-runner`
- Windows: `%APPDATA%\com.fraziersystems.nable-script-runner`
- Linux: `~/.local/share/com.fraziersystems.nable-script-runner`

To start from a clean first launch, delete that directory and remove both keychain entries. Keep the bundle identifier and keychain service name unchanged in code: changing either orphans existing users' saved key and history.

## Tests

Frontend tests run under Vitest and live beside the code they cover. Rust tests are in `#[cfg(test)]` modules in the same file as the code. CI runs both on every pull request and every push to `main`, along with the type-check build and the version-sync check.

## Continuous integration

`.github/workflows/ci.yml` runs on pull requests and pushes to `main`: version sync check, `npm run build`, `npm test` and `cargo test`. A change should be green there before it is merged.

## Commits and release notes

Commit messages follow the `type: summary` style already in the history (`feat:`, `fix:`, `docs:`, `chore:`). Release notes are generated from merged pull requests, grouped by label (see `.github/release.yml`): `enhancement` or `feature`, `bug` or `fix`, `documentation`, and `ignore-for-release` to leave a pull request out.

Publishing a release is covered in [RELEASING.md](RELEASING.md).
