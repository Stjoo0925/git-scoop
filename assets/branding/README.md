# Git Scoop branding

The mark combines a magnifying glass with three connected commit nodes: inspecting the path of code through history. Deep navy and cyan keep it recognizable in light and dark interfaces.

## Assets and usage

- `git-scoop-source.png`: original 1254 × 1254 RGBA artwork generated with the built-in imagegen tool on October 14, 2026.
- `git-scoop-512.png`: high-resolution logo for documentation and future presentation assets.
- `git-scoop-256.png`: embedded application logo and native window icon.
- `git-scoop-64.png`: compact logo for small surfaces.
- `git-scoop.ico`: Windows icon containing 16, 20, 24, 32, 40, 48, 64, 128, and 256 pixel images, used by the executable and Inno Setup installer.

All resized assets preserve the source alpha channel and use Pillow's Lanczos resampling. The source is retained so future exports do not accumulate resizing artifacts. These project assets are distributed under the project's MIT license.

The app caches its logo texture per egui context. The MSVC build compiles `crates/app/windows/app.rc` with Windows SDK `rc.exe` and links the icon resource into `git-scoop.exe`. Use the existing MSVC developer environment configured by `scripts/dev.ps1` or `scripts/package.ps1`. Start-menu shortcuts and the uninstall entry use the executable's embedded icon; the setup executable uses the same ICO.

## Generation prompt

Built-in imagegen, with `transparent_background: true`:

> Use case: logo-brand. Create a polished Windows desktop application icon for Git Scoop, a professional developer tool that explores code genealogy and first-parent Git history. One single centered icon, square canvas, transparent outside. A premium deep navy rounded-square tile with generous corner radius, crisp luminous teal/cyan branching commit path inside a stylized magnifying-glass/scoop shape. Three clearly visible circular commit nodes, thick clean strokes, recognizable at 32 pixels. Restrained subtle depth and small teal highlight, precise simple geometry, sophisticated calm developer-tool branding. No text, no letters, no watermark, no extra objects, no mockup, no multiple variants. Tile fills about 86 percent of canvas; consistent symmetrical padding. Deliver a single high-resolution RGBA icon.

Build integration references: [Cargo build scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html) and [Windows Resource Compiler](https://learn.microsoft.com/en-us/windows/win32/menurc/resource-compiler).
