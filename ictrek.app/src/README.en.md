# PhotoCraft

PhotoCraft is an open-source, Photoshop-comparable native image editor: one Rust engine with
several front ends. This app is the **browser build** — the same Rust code compiled to
WebAssembly, running entirely in your browser with no server-side compute.

## What it does

- **Layers**: layers, groups, masks, vector masks, clipping masks, layer styles, blend modes,
  adjustment layers, smart objects.
- **Painting and retouching**: brush engine with shape dynamics, mixer brush, clone stamp, healing
  brush, spot healing, content-aware fill, liquify.
- **Selection and paths**: lasso, magic wand, quick selection, colour range, pen and shapes, path
  operations, channels and quick mask.
- **Colour**: ICC colour management; RGB / Gray / CMYK / Lab; 8-bit, 16-bit and 32-bit float.
- **Type**: the full type tool, character and paragraph panels, type on a path, OpenType features.
- **Files**: PSD / PSB round trips with Photoshop, the native `.pcraft` format, and import/export
  for PNG, JPEG, TIFF, WebP, AVIF, HEIC, RAW and more.
- **Menus and shortcuts**: Photoshop's menu structure, panel layout and key bindings.

## Using it inside VOS

- Open PhotoCraft from the sidebar; it is embedded in the platform page. The UI language follows
  your browser and can be changed in **Edit › Preferences › Interface**.
- **Documents live in this browser tab only.** This first release opens and saves through the
  browser's own file picker and downloads; nothing is written to the NAS and there is no
  server-side session. Closing the tab discards unsaved work, exactly like closing a desktop window.
- Rendering prefers **WebGPU** and falls back to **WebGL2**. If the canvas misbehaves, append
  `?webgl` (force WebGL2) or `?cpu` (force the CPU canvas) to the URL and reload.

## Size and performance

- Large documents (thousands of pixels across, dozens of layers) use a lot of memory, and browser
  limits are lower than a desktop's. For very large files, downscale or flatten in the desktop
  build first.
- The first open downloads about 26 MB of wasm; later opens come from the browser cache.

## License

PhotoCraft is released under **MIT OR Apache-2.0**. The upstream project is
<https://github.com/storytold/photocraft>; copyright and third-party asset credits are served with
the site in `NOTICE`, `ATTRIBUTION.md` and `/licenses/`.
