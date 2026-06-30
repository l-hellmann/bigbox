# web (wasm) build

Browser build of the macroquad shell. Single-player, keyboard/mouse.

```sh
make web-serve        # build wasm + serve → http://localhost:8000
# or separately:
make web              # build → web/head2box.wasm
make serve PORT=9000  # serve web/ without rebuilding
```

## Files

- `index.html` — canvas (`#glcanvas`) + macroquad's JS loader. No wasm-bindgen.
- `mq_js_bundle.js` — macroquad's GL/audio/input glue, vendored from the
  `macroquad` crate (`js/mq_js_bundle.js`). Refresh it when bumping macroquad:
  `cp "$(find ~/.cargo/registry/src -name mq_js_bundle.js | head -1)" web/`.
- `head2box.wasm` — build output (git-ignored; produced by `make web`).

## Notes / caveats

- **Content** is embedded via `include_str!`, so there's nothing to fetch at
  runtime — no asset server, no CORS surprises.
- **No gamepad on web.** gilrs reaches the browser Gamepad API through
  wasm-bindgen, whose imports macroquad's plain loader can't provide; gilrs is
  native-only and the web build ships a no-op pad stub. Deferred.
- **No debug overlay on web.** The `debug` feature (egui) is native-only for the
  same wasm-bindgen reason — `build.sh` builds release without it.
- **RNG**: gameplay RNG is always seeded, so `getrandom` is never called; on
  wasm it's wired to a trivial stub backend (`main.rs`) to keep the wasm-bindgen
  footprint at zero.
