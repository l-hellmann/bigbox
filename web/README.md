# web (wasm) build

Browser build of the macroquad shell. Single-player; keyboard/mouse + gamepad.

```sh
make web-serve        # build wasm + serve → http://localhost:8000
# or separately:
make web              # build → web/head2box.wasm
make serve PORT=9000  # serve web/ without rebuilding
```

## Run config (URL query)

The seed and starting level are read from the page URL, so a URL reproduces an
exact run:

```
http://localhost:8000/?seed=123&level=arena
```

- `seed` — `u64` map seed (default `42`).
- `level` — `arena`, `arena-empty`, or anything else → the BSP dungeon (default).

Read via `quad-url` (sapp-jsutils JS bridge, no wasm-bindgen) in `resolve_run`;
the native equivalents are `H2B_SEED` / `H2B_LEVEL` (and the debug-build CLI).

## Files

- `index.html` — canvas (`#glcanvas`) + macroquad's JS loader. No wasm-bindgen.
- `mq_js_bundle.js` — macroquad's GL/audio/input glue, vendored from the
  `macroquad` crate (`js/mq_js_bundle.js`). Refresh it when bumping macroquad:
  `cp "$(find ~/.cargo/registry/src -name mq_js_bundle.js | head -1)" web/`.
- `sapp_jsutils.js`, `quad-url.js` — JS bridge for the URL-config read, vendored
  from the `sapp-jsutils` / `quad-url` crates. The bundle embeds its own
  sapp_jsutils but keeps the `js_object` helpers IIFE-private, so the standalone
  copy is loaded to expose them. Refresh alongside their crate versions.
- `quad-gamepad.js` — hand-rolled miniquad plugin (not a vendored crate) that
  reads the browser Gamepad API and exposes its numeric axes/buttons to the wasm.
  The web counterpart to gilrs; see the gamepad note below.
- `head2box.wasm` — build output (git-ignored; produced by `make web`).

## Notes / caveats

- **Content** is embedded via `include_str!`, so there's nothing to fetch at
  runtime — no asset server, no CORS surprises.
- **Gamepad on web** works, but *not* via gilrs — its web backend needs
  wasm-bindgen glue macroquad's loader can't provide. gilrs is native-only; on
  web, `quad-gamepad.js` reads the browser Gamepad API and passes the numeric
  axes/buttons across the wasm boundary (no wasm-bindgen, no marshaling), read
  by the `pad` module's wasm arm. Same twin-stick mapping as native. Browsers
  only surface a controller after you press a button on it (a privacy gate), so
  it won't register until then.
- **No debug overlay on web.** The `debug` feature (egui) is native-only for the
  same wasm-bindgen reason — `build.sh` builds release without it.
- **RNG**: gameplay RNG is always seeded, so `getrandom` is never called; on
  wasm it's wired to a trivial stub backend (`main.rs`) to keep the wasm-bindgen
  footprint at zero.
- **Cosmetic console error** `Plugin quad_url version mismatch … 0.1.2 / 65538`
  is harmless: the bundle compares quad-url.js's version *string* against the
  crate's *packed-int* `_crate_version`, so they never compare equal. Config
  still loads correctly.
