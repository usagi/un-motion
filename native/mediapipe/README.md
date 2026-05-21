# MediaPipe Native Backend

This directory contains the primary performance-target MediaPipe Native runtime
bridge.

Current product direction:

- Primary target: MediaPipe C++ DLL loaded through Rust FFI.
- Capturer runtime does not keep a MediaPipe Web fallback path.

The native backend remains source-controlled at the bridge level because it is
the desktop performance path. Normal app users still should not need Bazel,
MediaPipe source, or an OpenCV system installation.

The Rust ABI and loader live in `crates/un-motion-mediapipe-native`. This
directory contains only the UNMotion-owned C++ bridge, Bazel target, and pin
file. Generated DLLs, import libraries, downloaded models, Bazelisk, and
MediaPipe source checkouts are local artifacts ignored by git.

## Why This Is Experimental

MediaPipe C++ on Windows is operationally expensive:

- Bazel workspace setup
- MSVC compatibility patches
- Python/Bazel helper setup
- TensorFlow Lite external sources
- OpenCV runtime and FrameBuffer converter behavior
- model/task asset management

This is too much to expose to app users.

## Build

The official repository-local command surface is `xtask`. It reads
`native/mediapipe/mediapipe-pin.toml`, prepares the ignored
`third_party/mediapipe` checkout, downloads pinned model/Bazelisk/OpenCV
artifacts, copies the checked-in UNMotion bridge into the MediaPipe workspace,
applies the UNMotion vendor patches, and writes the DLL back to this directory.

```sh
cargo xtask mediapipe build-native
```

Heavy native builds default to half of the machine's physical CPU cores. Use
`--jobs N` to override this for a specific run.

The output DLL can be probed from UNMotion when it is available at:

```text
native/mediapipe/un-motion-mediapipe.dll
```

```sh
cargo xtask mediapipe native-probe -- --image path/to/image.png
```

Environment overrides:

- `UN_MOTION_MEDIAPIPE_DLL`
- `UN_MOTION_MEDIAPIPE_MODEL`
- `UN_MOTION_MEDIAPIPE_HAND_MODEL`

## Vendor Patch Notes

The Windows native build uses a repo-local pinned OpenCV runtime. This is
required by MediaPipe Tasks HolisticLandmarker because its graph currently runs
segmentation conversion calculators even when UNMotion does not request a
segmentation output.

The FrameBuffer converter patch is still kept because it is useful when testing
or bisecting OpenCV-disabled builds. MediaPipe's upstream FrameBuffer converter
only handles ROI rotations in 90-degree increments, but Pose/Hand/Face VIDEO
and LIVE_STREAM tracking feed arbitrary rotated ROIs back into
`ImageToTensorCalculator`. Ignoring those rotations makes static input drift and
produces unstable head/hand output.

UNMotion therefore replaces
`mediapipe/calculators/tensor/image_to_tensor_converter_frame_buffer.cc`
with a checked-in patch at
`native/mediapipe/patches/image_to_tensor_converter_frame_buffer.cc`. The
patch performs direct bilinear rotated ROI sampling for RGB/RGBA/GRAY frame
buffers and supports both zero and replicate border modes. Keep this patch in
sync with the MediaPipe pin when upgrading.

## License

MediaPipe is Apache-2.0. OpenCV may be BSD 3-Clause or Apache-2.0 depending on version. If any native binaries are distributed, update `THIRD_PARTY_NOTICES.md` and include upstream notices.
