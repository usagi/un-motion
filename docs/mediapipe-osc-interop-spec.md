# Waidayo OSC 相互運用メモ

この文書は local interop test で観測した Waidayo OSC mode を記録します。MediaPipe-over-OSC の実装仕様ではありません。

## 現在の製品ルール

Waidayo は **Sub Send Motion** を通常の VMC over OSC source として使います。

U.N. Motion は VMC/OSC input source で `/VMC/Ext/*` packet を受信します。この経路に Waidayo 固有 handshake は不要です。

Waidayo の **Send Motion** は product input として使いません。この stream は Waidayo 固有の `/MP/*` message を出すことがあり、receiver 固有の挙動や handshake semantics に依存しているように見えます。U.N. Motion では unsupported diagnostic observation としてだけ記録します。

## 観測した mode

### Sub Send Motion

Windows firewall prompt を許可した後、`192.168.13.13:39540` で観測しました。

- `frames=4800`
- `vmc_frames=4800`
- `mp_frames=0`
- `bones=31800`
- `blendshapes=42000`
- `decode_errors=0`

captured stream には次のような標準 VMC address が含まれていました。

- `/VMC/Ext/Set/Eye`
- `/VMC/Ext/Blend/Val`
- `/VMC/Ext/Bone/Pos`

これは VSeeFace や Warudo 方式の VMC receiver と互換の経路です。Waidayo input profile では VMC/OSC source を設定し、Waidayo Sub Send Motion の送信先をその host / port に向けます。

Windows では、特定 interface bind で LAN packet を受けられない場合の fallback として `0.0.0.0:<port>` が有効です。ただし VMC/OSC source はユーザーが設定した host / port をそのまま bind するのが基本です。

### Send Motion

`127.0.0.1:39540` での local capture では次のような message を観測しました。

- `/MP/AUX`
- `/MP/BS`
- `/MP/FACELM`
- `/MP/LH`
- `/MP/RH`
- `/MP/POSEFULL`

これらは VMC bone / blendshape packet ではありません。U.N. Motion の supported external input dialect として扱ってはいけません。将来研究する場合も、handshake、座標系、payload semantics が証明されるまで runtime VMC/OSC receiver から分離します。

## Warudo / VSeeFace との扱い

この workflow では Warudo と VSeeFace を VMC/OSC peer として扱います。

- Waidayo -> U.N. Motion: Waidayo Sub Send Motion -> VMC/OSC source。
- U.N. Motion -> VSeeFace / Warudo: VMC output。
- VSeeFace / Warudo -> U.N. Motion: 実験なら VMC/OSC probe、runtime profile なら VMC/OSC source。

ユーザー向け UI や architecture docs で、これを MP/OSC support と説明してはいけません。
