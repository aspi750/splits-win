# Splits Coach (Windows / Tauri v2)

Native Windows port of **Splits Coach** (`© 2026 A750LABS. Based on Splits by Bytefabrika.`), implementing the cross-platform `CONTRACT.md` specification 1:1 with `splits-mac`.

## Architecture
- **Backend (`src-tauri/`)**: Rust + Tauri v2, bundled SQLite (`rusqlite`), Windows Credential Manager (`keyring`), Garmin Connect Personal API client, deterministic `ReadinessEngine`, `CoachPacketBuilder`, multi-round Gemini 4 Hydrogen tool-calling client, System Tray, and Toast Notifications.
- **Frontend (`src/`)**: React 19 + TypeScript + Vite.

## Development
```bash
npm install
npm run build
cd src-tauri && cargo test
```
