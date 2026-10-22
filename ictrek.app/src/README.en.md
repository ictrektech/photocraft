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

- Open PhotoCraft from the sidebar; it is embedded in the platform page and follows live VOS language changes.
- Editing happens in the browser tab. Save before closing: download to this computer, write to a VOS-authorized directory, or export a finished image to AI Album.
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

## Image sources and saving

File → Open offers this computer, VOS, and AI Album. Saving offers a browser download or a VOS directory.
Grant PhotoCraft directories through VOS App management → Data access authorization, then refresh the picker.
Public and current-user directories are supported; read-only grants cannot be saved to. An admin personal-volume allocation is not required.
AI Album uses the current VOS identity without another password or API key. AI Album must be installed with VOS login enabled.
Only that account's accessible images are shown. Images open as editable copies; save to this computer or VOS to keep the album original.
The UI follows the VOS language on startup and on live language changes without reloading.

### Export finished images to AI Album

Save and Export retain browser download and VOS destinations. For PNG, JPEG and WebP output,
the destination chooser also offers **Export image to AI Album**. The same user session uploads
the encoded result, checks the returned asset through the album API, and only then confirms success.
Original album images are preserved. Layered PSD/PCraft projects stay on this computer or VOS.
Failed asynchronous saves report the failure and restore the document's unsaved state so it can be retried.
