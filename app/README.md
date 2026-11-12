# Post: the desktop app

The Flutter app for Post (Linux first). Building, running it and creating an
account are in the main [README](../README.md#run-it-on-your-computer).

| Path | What |
|---|---|
| `lib/screens/` | The screens: welcome, the main view, chat, side panel, dialogs, settings |
| `lib/state/` | What the signed-in app knows, kept current from the core's events |
| `lib/ui/` | Shared controls, the window frame, message formatting |
| `lib/src/rust/` | Generated bindings to `crates/post-ffi` (do not edit) |
| `linux/` | The GTK runner, its custom window frame, and launcher files |
| `integration_test/` | Screenshot and timing tests against a throwaway server |
| `tool/` | `screens.sh` and `timing.sh`, which run those tests |
| `assets/` | Fonts (OFL, licences beside them) and the placeholder logo |
