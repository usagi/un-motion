# Capturer パイプライン規約

この文書は Capturer runtime の正式な処理経路を定義する。

## 正式経路

Capturer の処理経路は次の 1 本。

```text
Input -> Engine -> UNMotionFrame -> Modifier -> Output
```

MediaPipe Native の場合、Engine は推論と post-process の両方を含む。

```text
Image input -> MediaPipe Native -> MediaPipe post-process -> UNMotionFrame
           -> Modifier -> Output(UNMF/Z, VMC/UDP, VRC (VRCFT) / OSC)
```

VMC や iFacialMocap などの protocol 入力では、decoder が Engine 境界として動作し、入力 protocol を `UNMotionFrame` へ正規化する。

```text
VMC input          -> VMC decoder          -> UNMotionFrame -> Modifier -> Output
iFacialMocap input -> iFacialMocap decoder -> UNMotionFrame -> Modifier -> Output
```

`UNMotionFrame` が内部の frame 契約。`UNMF/Z` は Zenoh でその frame を publish する transport、`VMC/UDP` は同じ Modifier 適用後の `UNMotionFrame` を OSC packet に変換する transport、`VRC (VRCFT) / OSC` は同じ Modifier 適用後の Face signal を VRChat OSC Avatar Parameters に変換する transport。

## 各段の責務

- Input は camera、image、video、protocol packet の取得を担当する。
- Engine は推論または protocol decode を担当し、`UNMotionFrame` を出す。
- MediaPipe post-process は MediaPipe Engine 境界の一部。Engine 内部構造を Output へ漏らさない。
- Modifier は `UNMotionFrame` だけを読み書きする。
- Output は Modifier 適用後の `UNMotionFrame` だけを読む。
- UNMF/Z は Modifier 適用後の `UNMotionFrame` を publish する。
- VMC/UDP は最後の境界で Modifier 適用後の `UNMotionFrame` を VMC OSC へ変換する。
- VRC (VRCFT) / OSC は最後の境界で Modifier 適用後の `UNMotionFrame` の Face signal を VRChat OSC Avatar Parameters へ変換する。

## 禁止する実装

新しい実装で次の経路を追加しない。

- Output が MediaPipe raw output や post-process 専用データを直接読む。
- VMC が `UNMotionFrame` を迂回して送信される。
- `UNMF/Z` を `UNMotionFrame` とは別の内部 branch として扱う。
- VRC (VRCFT) / OSC が MediaPipe raw blendshape や VMC packet を直接読む。
- bone subset、smoothing、mirror、calibration を Modifier より前で UNMF/Z と VMC に別々に適用する。
- 通常の Capturer 経路に runtime mux / fusion state を戻す。

## 複数 source の扱い

古い runtime mux / fusion 経路は Capturer から削除済み。

将来の複数 source 合成は、通常の単一 Capturer 経路の外側で扱う。例えば UNMotionSynthesizer のような上位層で、各 source を `UNMotionFrame` stream として受け取り、優先度、blend、TTL などを決める。
