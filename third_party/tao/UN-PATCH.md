# Windows input reentrancy correction

Base: crates.io tao 0.35.2, original Apache-2.0 license retained.
Applied without source modifications: upstream commit
c704261c519c58cfdd0bc2d58ba24e06a0b71c92 (tauri-apps/tao PR #1215).

https://github.com/tauri-apps/tao/pull/1215

Moves PeekMessageW outside keyboard-builder, keyboard-layout and IME locks.
This addresses the live UNMotion startup self-deadlock captured on 2026-10-23.
The focused backport avoids unrelated API/platform changes in Tao 0.36+.
Remove this patch when the Supervisor runtime adopts an upstream release
containing this correction, after the keyboard/focus regression is checked.
