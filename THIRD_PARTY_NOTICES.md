# Third Party Notices

U.N. Motion is MIT licensed. Some runtime components, development dependencies, models, and optional native runtime files are third-party software.

This notice is a release checklist as well as an attribution file. If a release package includes a binary or model listed here, keep the corresponding upstream license and notices with the distribution.

## MediaPipe

U.N. Motion's primary pose engine uses MediaPipe Native through the local Rust/C++ bridge.

- Components: MediaPipe C++ source, MediaPipe task models, and local U.N. Motion bridge code.
- Upstream: https://github.com/google-ai-edge/mediapipe
- License: Apache-2.0

The app may bundle pose, face, hand, and holistic landmarker task models under `models/*.task`. These model files are third-party MediaPipe model assets and are not relicensed by U.N. Motion.

Release action:

- Include Apache-2.0 license and MediaPipe notices when task models or MediaPipe native binaries are bundled.
- Keep model provenance in `native/mediapipe/mediapipe-pin.toml`.
- Bundled license file: `LICENSES/MediaPipe-Apache-2.0.txt`.

## ccap-rs

U.N. Motion vendors `ccap-rs` for the Windows DirectShow webcam backend.

- Project: ccap-rs / CameraCapture
- Upstream: https://github.com/wysaid/CameraCapture
- License: MIT
- Local path: `third_party/ccap-rs`
- Bundled license file: `LICENSES/ccap-rs-MIT.txt`.

The vendored source is part of this repository through `[patch.crates-io]`. Keep its upstream MIT attribution when distributing source or binaries that use the DirectShow backend.

## Tauri / WebView2

The desktop app uses Tauri and the platform WebView runtime. On Windows, that means Microsoft Edge WebView2.

- Tauri Rust crates and `@tauri-apps/api`: Apache-2.0 OR MIT
- Microsoft Edge WebView2 Runtime: Microsoft-distributed platform runtime.

The Windows runtime requirement is WebView2. The app does not bundle Chromium itself.

## Frontend Libraries

- Svelte: MIT
- svelte-i18n: MIT
- lucide-svelte: ISC
- Vite / Rolldown / build-time frontend dependencies: see `apps/un-motion-supervisor/package-lock.json`.

`lightningcss` and its platform packages are MPL-2.0 in the current lockfile. They are build tooling dependencies; if a release process starts bundling frontend tool binaries or source packages, include their notices explicitly.

## OpenCV

OpenCV may be bundled as `opencv_world3410.dll` when building the Windows MediaPipe Native package.

- Project: OpenCV
- Upstream: https://opencv.org/
- Version used by current native pin: 3.4.10
- License: BSD 3-Clause for OpenCV 3.x

Release action:

- If `opencv_world3410.dll` is present in the release package, include the matching OpenCV 3.4.10 license and notices.
- Avoid optional GPL/nonfree codec stacks unless they are explicitly reviewed.
- Bundled license file: `LICENSES/OpenCV-3.4.10-BSD-3-Clause.txt`.

## U.N. Common / U.N. i18n

The Supervisor uses `@usagi.network/un-i18n-svelte`, which is a USAGI.NETWORK MIT package.

- Package: `@usagi.network/un-i18n-svelte`
- License: MIT
- Source family: U.N. Common

Do not keep local `.tgz` package artifacts in this repository unless `package.json` explicitly references them.

## Lockfiles

Rust and npm lockfiles are tracked intentionally. They are used to audit the exact dependency set for a release.

- Rust dependencies: `Cargo.lock`
- Frontend dependencies: `apps/un-motion-supervisor/package-lock.json`
- Generated release report: `THIRD_PARTY_DEPENDENCIES.md`

The current dependency graph includes permissive licenses and weak-copyleft choices such as EPL-2.0 OR Apache-2.0, MPL-2.0, IJG, and CDLA-Permissive-2.0. `cargo xtask license-report` generates the lockfile-based report, and `cargo xtask make-release-package` includes it in the package root. Before a public binary release, review that report and include any required notices that are not already covered by this file.
