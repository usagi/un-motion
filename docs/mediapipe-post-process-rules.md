# MediaPipe Post-process ルール一覧

UNMotion の MediaPipe post-process は、MediaPipe の raw landmark、world landmark、face matrix、blendshape を `MotionSignal` に変換する段で複数の補正を行う。pose fixture や live capture との比較では、フィルタとは別にこの層の影響を切り分ける必要がある。

対象コード:

- `crates/un-motion-engine-mediapipe-post-process/src/lib.rs`
- `apps/un-motion-supervisor/src-tauri/src/lib.rs`
- `crates/un-motion-output-vmc/src/lib.rs`

## 現在の外部 switch

現時点で runtime から直接触れる主な post-process 関連設定。

| 設定 | 既定 | 影響 |
|---|---:|---|
| `components.post_process.kind = "media-pipe-default"` | on | MediaPipe 出力を UNMotion signal に変換する。 |
| `components.post_process.kind = "none"` | off | raw diagnostic frame を作る。motion signal は出さないため、VMC 比較用の「信号あり raw」ではない。 |
| `modifier.head_enabled` | true | head signal 生成、face matrix 由来 head、pose 由来 head の使用。 |
| `modifier.face_enabled` | true | eye/face blendshape signal 生成。 |
| `modifier.hands_enabled` | false | hand wrist、open/pinch/palm/finger signal 生成。 |
| `modifier.arms_ik_enabled` | false | Arms。pose arm signal と hand 由来 IK arm signal 生成。UI では `Arms` と表示する。 |
| `modifier.torso_enabled` | false | Torso。hip から shoulder までの torso signal 生成。 |
| `modifier.legs_enabled` | false | Legs。hip/knee/ankle の leg signal 生成。 |
| `modifier.feet_enabled` | false | Feet。ankle/heel/foot index の foot signal 生成。 |
| `modifier.min_landmark_confidence` | 0.55 | pose/head/hand/arm signal の採用閾値。 |
| `modifier.camera_diagonal_view_angle_deg` | 70 | hand wrist の疑似 3D 推定で使う camera model。 |
| `modifier.mirror_mode` | `normal` | VMC 座標補正後の user mirror / side swap。 |

`media_pipe_post_process_config_with_source` は profile runtime の `RuntimeSelection` から `MediaPipePostProcessConfig` を作る。UI の主 part は `Head`、`Face`、`Hands`、`Arms`、`Torso`、`Legs`、`Feet` の英語ラベルを使い、説明で日本語の意味を補う。

Post-process 画面では、VMC/Warudo 互換寄せや評価時の切り分けに影響する rule を `modifier.postProcessRules` として個別に ON/OFF できる。既定値は全 ON で現行互換。

## 解剖学的制約

UI では `MediaPipe Post-process > 解剖学的制約` として表示する。Supervisor の field update path は `runtime_selection.modifier.post_process_rules.anatomical_constraints`、profile TOML の保存キーは `runtimeSelection.modifier.postProcessRules.anatomicalConstraints`。既定値は `true`。

このスイッチは、MediaPipe が出すランドマークをそのまま Humanoid bone rotation として信用せず、人体として成立しにくい出力を抑える上位スイッチである。現時点では以下を同じ制約群として扱う。

- local rotation の保守的な ROM clamp。
- 見失い復帰時の recovery easing。
- 1フレーム単位の過大な angular jump limit。
- face/pose head source の切り替わり blend。

実装上の基準は `crates/un-motion-engine-mediapipe-post-process/src/lib.rs` の `apply_anatomical_constraints` と `MotionStabilizer`。VRM Humanoid は実人体の骨や軟部組織を完全には表現できないため、ここでは「疾患や極端な柔軟性を含む最大ROM」ではなく、Webcam推定の誤爆を防ぐための保守的な上限として使う。

| 対象 | 実装の考え方 | 論拠 |
|---|---|---|
| Shoulder / UpperArm | 肩は多軸関節で可動域は大きいが、肩甲骨と軟部組織に拘束される。UpperArm は広いswingを許しつつ、上腕長軸まわりの内外旋twistを制限する。 | NCBI Bookshelf `Glenohumeral Joint` は屈曲180度、伸展45-60度、内旋70-90度、外旋90度、外転150度を通常範囲として整理している。 |
| Elbow / LowerArm | 肘はヒンジ性が強く、屈伸と前腕回内・回外を分けて考える。LowerArm に任意rollを背負わせない。 | NCBI Bookshelf `Elbow Collateral Ligaments` は健康成人のROMを屈曲130-154度、回内75-85度、回外80-104度としている。 |
| Wrist / Hand | 手首は屈曲・伸展・橈屈・尺屈が主で、前腕回内・回外を手首単体へ押し込まない。 | StatPearls `Wrist Arthritis` は手首ROMを屈曲65-80度、伸展55-75度、尺屈30-45度、橈屈15-25度としている。 |
| Knee / LowerLeg | 膝は屈伸が主で、軸回旋は屈曲に伴う副次的な動きとして狭く扱う。LowerLeg の長軸twistを制限する。 | NCBI Bookshelf `Knee` は膝を主に sagittal plane の屈曲・伸展を行う荷重関節とし、内外旋などの二次運動は軟部組織に制限されると整理している。 |
| Ankle / Foot | 足関節・足部は底屈/背屈、内反/外反が主で、足先ボーンの軸回りrollを無制限にしない。 | NCBI Bookshelf `Ankle Joint` と `Biomechanics of the ankle` は足関節複合体の主要運動を plantarflexion/dorsiflexion と inversion/eversion として整理している。 |
| Thumb | CMC は鞍関節で flexion/extension と abduction/adduction が主、軸回旋は副次的。MP/IP は握り込みで大きく屈曲できる。 | `A Thumb Carpometacarpal Joint Coordinate System Based on Articular Surface Geometry` は CMC の報告ROMを flexion/extension 約53度、abduction/adduction 約42度、axial rotation 約17度としている。 |
| Fingers | MCP は屈伸に加えて外転/内転、PIP/DIP は主に屈伸。PIP/DIP の横折れや過大rollは抑える。 | NCBI Bookshelf `The Musculoskeletal Examination` は手指ROMの診察基準を示し、親指IP 90度屈曲などを例示している。 |

### VRM T/Twf 基本姿勢と肘ヒンジ前提

UNMotion の T-wrist-front は、VRM Humanoid の T-pose を「腕は左右、手首/手の平はfront」と解釈する。ただし人体の実骨格では、T/Twf の見た目を作る時点で肩関節の外転・外旋が入っており、肘の屈伸軸はモデル座標の単純な up/front 軸とは一致しない。

したがって、LowerArm を「T-pose identity から任意方向へ swing できるボーン」と扱うと誤りになる。肘は humeroulnar joint を中心とするヒンジ性の屈伸が主で、手の平向きは前腕の radioulnar pronation/supination で作られる。実装では、VRM がモデル固有の実ヒンジ軸を持たない制約を認めた上で、LowerArm local rotation を以下の2成分に分けて制限する。

- swing: 肘屈伸相当。任意方向の肘折れを防ぐ。
- twist: 前腕の回内・回外相当。手首/手の平向きの変化をLowerArm rollへ押し込みすぎない。

これは完全な人体シミュレーションではない。目的は、MediaPipe 再捕捉や奥行き誤差で「肘だけが本来曲がれない方向へ曲がる」「前腕rollが無制限に飛ぶ」出力を防ぐことである。

参照:

- https://www.ncbi.nlm.nih.gov/books/NBK537018/
- https://www.ncbi.nlm.nih.gov/books/NBK532948/
- https://www.ncbi.nlm.nih.gov/books/NBK518967/
- https://www.ncbi.nlm.nih.gov/books/NBK560907/
- https://www.statpearls.com/point-of-care/31409
- https://www.ncbi.nlm.nih.gov/books/NBK500017/
- https://www.ncbi.nlm.nih.gov/books/NBK545158/
- https://pmc.ncbi.nlm.nih.gov/articles/PMC4994968/
- https://pmc.ncbi.nlm.nih.gov/articles/PMC3594374/
- https://www.ncbi.nlm.nih.gov/books/NBK272/

## ルール一覧

### 1. Holistic 出力選択

所在: `native_mediapipe_output_signals`

入力:

- `native.holistic.pose`
- `native.holistic.left_hand`
- `native.holistic.right_hand`
- `native.holistic.face`
- fallback として `native.pose`、`native.hands`、`native.face`

処理:

Holistic 側に pose/hand/face のいずれかがあれば holistic 出力を優先し、left/right hand を `NativeHands` に詰め直す。無ければ非 holistic の pose/hands/face を使う。

出力:

- 後段すべての入力セットが変わる。

目的:

- MediaPipe holistic path を desktop の主経路として扱うため。

副作用:

- holistic と非 holistic の混在比較が見えにくい。
- hand assignment は `left_hand`、`right_hand` の slot をそのまま信じる。

現状の ON/OFF:

- `media_pipe_holistic_enabled` は MediaPipe 実行側の選択。post-process 内部では個別に切れない。

設定化候補:

- 低。まず metadata notes にどちらを採用したか出す方が有益。

### 2. pose 由来の head

所在: `head_signals_from_pose`、`head_signals_from_pose_world`

入力:

- pose landmarks: nose `0`、eyes `2/5`、ears `7/8`
- pose world landmarks: nose、eyes、ears、shoulders `11/12`

処理:

- world landmarks が十分なら world 由来を優先。
- yaw は nose と ears の左右関係から計算。
- pitch は face forward と shoulder/eye の相対高さを混ぜる。
- roll は ears または eyes の傾きから計算。
- 各値は `[-1, 1]` に clamp。

出力:

- `head.yaw`
- `head.pitch`
- `head.roll`

目的:

- face matrix が無い、または不安定な時にも head signal を出す。

副作用:

- 肩や耳の landmark が不安定な高速の手動作では head pitch が姿勢変化に引っ張られる。
- world landmark の座標系仮定が VMC 側の見た目に強く出る。

現状の ON/OFF:

- `head_enabled = false` で head 全体を切ることは可能。
- pose-head だけを切る設定は無い。

設定化候補:

- 高。`rules.head_from_pose` として切りたい。

### 3. face matrix 由来の head

所在: `push_face_signals`、`normalized_face_rotation`

入力:

- face transform matrix
- face confidence

処理:

- 3x3 相当の回転行列を正規化。
- yaw/pitch/roll を取り出し、固定係数で正規化。
- confidence は `face.confidence.max(0.75)` で最低 0.75 に底上げ。

出力:

- `head.yaw`
- `head.pitch`
- `head.roll`

目的:

- face matrix 由来の head 回転を VMC/Warudo の head movement に寄せる。

副作用:

- confidence の底上げで、弱い face 由来 head が強く見える。
- `head_from_pose` との矛盾を後続 rule が補正するため、実際の由来が見えにくい。

現状の ON/OFF:

- `head_enabled = false` で切れるが、pose head も同時に切れる。

設定化候補:

- 高。`rules.head_from_face_matrix` として切りたい。

### 4. head の reconcile

所在: `reconcile_head_signals_with_pose`、`head_signals_are_saturated`

入力:

- face matrix 由来の `head.*`
- pose 由来の `head.*`

処理:

- face head が saturated と判定されたら pose head に置換。
- face と pose の符号が矛盾し、両方の絶対値が 0.08 以上なら、face の絶対値を保って pose の符号へ補正。
- confidence は face と pose の低い方へ落とす。

出力:

- `head.yaw`
- `head.pitch`
- `head.roll`

目的:

- MediaPipe face matrix の左右符号や飽和を Warudo/VMC で自然に見える向きへ寄せる。

副作用:

- どちらが正しいかではなく「符号だけ」補正するため、横向きや速い動きで過補正の可能性。
- head signal の由来が frame から分からない。

現状の ON/OFF:

- 個別には切れない。

設定化候補:

- 最優先。`rules.head_reconcile`。

### 5. eye / face signal

所在: `push_face_signals`

入力:

- face blendshapes
- face landmarks / confidence

処理:

- `eyeLookOutLeft` などの blendshape 差分から `eye.*.yaw/pitch` を作る。
- face landmark はあるが eye blendshape が無い場合、eye を 0.0 として出す。
- `_neutral` 以外の blendshape を `face.{name}` として出す。

出力:

- `eye.left.yaw`
- `eye.right.yaw`
- `eye.left.pitch`
- `eye.right.pitch`
- `face.*`

目的:

- VMC blendshape route に渡せる face/eye signal を作る。
- eye が無い時に目線をニュートラルに保つ。

副作用:

- neutral eye fallback は「不明」と「正面」を区別しない。
- face blendshape confidence は現在ほぼ 1.0 として出る。

現状の ON/OFF:

- `face_enabled = false` で face/eye 全体は切れる。
- neutral eye fallback だけは切れない。

設定化候補:

- 中。`rules.neutral_eye_fallback` は評価時に切りたい。

### 6. hand camera target

所在: `push_hand_signals`、`hand_camera_target`、`camera_model`

入力:

- hand normalized landmarks
- wrist `0`
- middle MCP `9`
- palm center `0/5/9/13/17`
- input width/height
- camera diagonal FOV

処理:

- wrist と middle MCP の投影距離から疑似 depth meters を推定。
- normalized palm center を camera ray に変換。
- side bias、depth-based lateral scale、固定 Y/Z 係数で `hand.{side}.wrist.{x,y,z}` を作る。
- 出力は `[-1, 1]` に clamp。

出力:

- `hand.left.wrist.x/y/z`
- `hand.right.wrist.x/y/z`

目的:

- MediaPipe hand landmark だけで VMC の hand target に近い 3D 的な wrist 位置を作る。

副作用:

- FOV、入力解像度、手の大きさの仮定が強い。
- 手を大きく振る動作では lateral scale と clamp が振幅を制限する可能性。
- 手首そのものではなく palm center を ray 化するため、回転・開閉で位置が動く。

現状の ON/OFF:

- `hands_enabled = false` で hand 全体は切れる。
- hand wrist 位置推定だけは切れない。

設定化候補:

- 最優先。`rules.hand_camera_target`。raw normalized wrist を出す比較モードも欲しい。

### 7. hand shape / orientation

所在: `push_hand_signals`、`hand_open`、`finger_pinch`、`palm_roll`、`wrist_rotation_signals`、`push_finger_curl_signals`、`push_finger_spread_signals`

入力:

- hand world landmarks があれば world、無ければ normalized landmarks
- wrist、finger tips、MCP/PIP/DIP、palm basis points

処理:

- open/pinch は landmark 間距離を palm scale で正規化。
- palm roll は index MCP と little MCP の 2D 角度。
- wrist pitch/yaw/roll と palm basis vectors は wrist/middle/index/little から算出。
- finger curl/spread は finger chain length と角度から算出。
- 多くの値は固定係数で正規化し `[-1, 1]` または `[0, 1]` に clamp。

出力:

- `hand.{side}.open`
- `hand.{side}.pinch`
- `hand.{side}.palm.roll`
- `hand.{side}.palm.forward.*`
- `hand.{side}.palm.across.*`
- `hand.{side}.palm.normal.*`
- `hand.{side}.wrist.pitch/yaw/roll`
- `hand.{side}.{finger}.curl`
- `hand.{side}.{finger}.{joint}.curl`
- `hand.{side}.{finger}.spread`

目的:

- VMC の手・指・palm 表現に近い scalar/basis signal を作る。

副作用:

- 手話の高速動作では finger curl/spread がノイズ源になりやすい。
- world/normalized の切替で座標解釈が変わる。
- palm basis の符号は後段 coordinate correction に依存する。

現状の ON/OFF:

- `hands_enabled = false` で hand 全体は切れる。
- `include_fingers` は内部的に存在するが desktop では `hands_enabled` と同じで、独立設定は無い。
- palm/wrist orientation だけを切る設定は無い。

設定化候補:

- 中。`rules.finger_derived` と `rules.hand_orientation` に分けると評価しやすい。

### 8. pose 由来の arm

所在: `push_arm_pose_signals`、`push_arm_pose_side_signals`、`pose_arm_point`、`arm_bend_signal`

入力:

- pose landmarks: shoulders `11/12`、elbows `13/14`、wrists `15/16`
- pose world landmarks が十分なら world を優先

処理:

- world 使用時は y/z を反転。
- normalized 使用時は x を `x - 0.5`、y を `0.5 - y`、z を `-z`。
- shoulder/elbow/wrist position、elbow bend、upper/lower 2D angle を出す。
- confidence は shoulder/elbow/wrist の平均。

出力:

- `arm.{side}.shoulder.x/y/z`
- `arm.{side}.elbow.x/y/z`
- `arm.{side}.wrist.x/y/z`
- `arm.{side}.elbow.bend`
- `arm.{side}.upper.angle`
- `arm.{side}.lower.angle`

目的:

- MediaPipe pose から腕の VMC 用 signal を直接作る。

副作用:

- MediaPipe pose wrist は hand wrist より荒いことがある。
- world landmark のスケールと向きは VMC の腕長・肩位置とは一致しない。
- 腕交差や体幹回転で elbow が飛ぶと VMC 側に大きく出る。

現状の ON/OFF:

- `arms_ik_enabled = false` で arm from pose も hand IK arm も一緒に切れる。
- arm from pose だけのスイッチは無い。

設定化候補:

- 高。`rules.arm_from_pose`。

### 8a. pose 由来の torso

所在: `push_torso_signals`

入力:

- pose landmarks: shoulders `11/12`、hips `23/24`
- pose world landmarks が十分なら world を優先

処理:

- `Torso` は hip を基準に shoulder までを扱う。
- shoulder は torso が所有し、Arms は shoulder を anchor として読む、という意味づけにする。

出力:

- `torso.left.shoulder.x/y/z`
- `torso.right.shoulder.x/y/z`
- `torso.left.hip.x/y/z`
- `torso.right.hip.x/y/z`

目的:

- 体幹と肩の位置を arm output から分離する。

副作用:

- 出力側では `Chest` に接続する。上半身の横向き・ひねりは `Chest` に載せ、`Hips` へは波及させない。

現状の ON/OFF:

- `torso_enabled`。

設定化候補:

- 主 part として実装済み。細かい rule 化は後でよい。

### 8b. pose 由来の legs

所在: `push_leg_signals`

入力:

- pose landmarks: hips `23/24`、knees `25/26`、ankles `27/28`
- pose world landmarks が十分なら world を優先

出力:

- `leg.left.hip/knee/ankle.x/y/z`
- `leg.right.hip/knee/ankle.x/y/z`

目的:

- hip から ankle までの脚 signal を分離する。

副作用:

- 上半身中心のカメラでは knees/ankles の visibility が低くなりやすい。
- 出力側では `Hips`、`LeftUpperLeg`、`RightUpperLeg`、`LeftLowerLeg`、`RightLowerLeg` に接続する。`Hips` は親階層として下半身全体に波及するため、webcam MediaPipe では hip ランドマーク由来の回転を弱く減衰して載せる。

現状の ON/OFF:

- `legs_enabled`。

設定化候補:

- 主 part として実装済み。

### 8c. pose 由来の feet

所在: `push_feet_signals`

入力:

- pose landmarks: ankles `27/28`、heels `29/30`、foot index `31/32`
- pose world landmarks が十分なら world を優先

出力:

- `foot.left.ankle/heel/index.x/y/z`
- `foot.right.ankle/heel/index.x/y/z`

目的:

- ankle から heel/foot index までの足先 signal を Legs から分離する。

副作用:

- MediaPipe の `ankle` と `heel` は別 landmark。足先が画面外になると confidence で落ちやすい。
- VMC 出力側では `LeftFoot`、`RightFoot`、`LeftToes`、`RightToes` に接続する。

現状の ON/OFF:

- `feet_enabled`。

設定化候補:

- 主 part として実装済み。

### 9. hand 由来の arm IK

所在: `push_arm_ik_from_hand_signals`、`push_arm_ik_side_from_hand_signals`、`solve_arm_ik`

入力:

- `hand.{side}.wrist.x/y/z`
- `hand.{side}.present`
- 既存の `arm.{side}.shoulder/elbow/wrist.*`

処理:

- arm pose signal が既にそろっている側は何もしない。
- arm pose が無い時、hand wrist から肩を固定値で置き、2 bone IK で elbow を推定。
- 上腕長 0.48、前腕長 0.46 の固定値。
- preferred elbow plane は通常 `y=-0.55,z=-0.3`。

出力:

- arm from pose と同じ `arm.{side}.*`

目的:

- pose arm が欠落しても hand modifier から腕を動かす。

副作用:

- 肩が固定値なので、実際の肩・体幹とはずれる。
- hand wrist 推定の誤差が elbow/shoulder へ増幅される。
- 「手首だけで振っている」見た目の原因になり得る。

現状の ON/OFF:

- `arms_ik_enabled = false` で切れるが、arm from pose も同時に切れる。

設定化候補:

- 最優先。`rules.arm_ik_from_hands`。

### 10. 右手交差 heuristic

所在: `push_arm_ik_side_from_hand_signals`

入力:

- right hand wrist x/y/z

処理:

- side が right かつ `x > 0.02` かつ `y < 0.0` なら crossed とみなす。
- shoulder を通常より変え、wrist z に `+0.25`。
- preferred elbow plane を `x = side_sign * -0.25, y = -0.65, z = -0.55` に変える。

出力:

- `arm.right.*`

目的:

- 右手が体の左側へ交差する時の IK elbow/shoulder を破綻しにくくする。

副作用:

- 条件が右手専用で、両手交差には非対称。
- `x/y` 閾値だけなので、通常動作を交差と誤判定する可能性。

現状の ON/OFF:

- 個別には切れない。`arms_ik_enabled = false` で間接的に切れる。

設定化候補:

- 高。`rules.crossed_hand_heuristic`。

### 11. 座標補正と mirror

所在: `apply_modifier_transforms`、`apply_vmc_coordinate_correction`、`apply_user_horizontal_mirror`、`swap_signal_side`

入力:

- 生成済み `MotionSignal`
- `mirror_mode`

処理:

- face signal 以外に VMC 座標系向けの符号反転を適用。
- 反転対象は `head.yaw`、全 `.yaw`、`.wrist.x`、`.shoulder.x`、`.elbow.x`、palm basis の一部、upper/lower angle。
- `mirror-output` または `swap-sides` では user horizontal mirror を追加適用。
- `swap-sides` では `.left.` と `.right.` を名前上で入れ替える。

出力:

- ほぼ全 signal の符号または side 名。

目的:

- VMC/Warudo の見た目に合う座標方向へ合わせる。
- ユーザーの mirror 設定を反映する。

副作用:

- raw MediaPipe 座標との比較時に符号が見えにくい。
- face signal は除外されるため、head と face blendshape の座標思想が混ざる。
- `swap-sides` は符号補正後に side 名を入れ替える。

現状の ON/OFF:

- `mirror_mode` は変更可能。
- VMC coordinate correction 自体は個別には切れない。

設定化候補:

- 最優先。`rules.coordinate_correction`。比較用に raw signal coordinate を出せる必要がある。

### 12. 最終 clamp と confidence 正規化

所在: `process_native_output_with_sequence`

入力:

- 生成済み `MotionSignal`

処理:

- scalar value を `[-1, 1]` に clamp。
- confidence を `[0, 1]` に clamp。
- frame source confidence は signal confidence の平均。

出力:

- frame.signals 全体。

目的:

- VMC 出力に渡せる範囲へ収める。

副作用:

- どの signal が本来範囲外だったか消える。
- 大きな手振りで saturation を隠す。

現状の ON/OFF:

- 切れない。

設定化候補:

- 中。安全用途としては常時必要だが、評価ログには pre-clamp 値が欲しい。

## VMC output との関係

Post-process は VMC を直接作らない。Post-process の成果物は `UNMotionFrame` であり、
Modifier 後の同じ frame を `crates/un-motion-output-vmc/src/lib.rs` が VMC OSC に変換する。
ここにも post-process ではないが、見た目に影響する変換がある。

- head yaw/pitch/roll は blendshape route と bone orientation の両方に関与する。
- hand palm normal/forward は手の向き推定に使われる。
- arm shoulder/elbow/wrist は腕ボーン推定に使われる。
- `LeftLowerArm` / `RightLowerArm` は elbow→wrist の幾何で主方向を作り、`palm.normal`
  は elbow→wrist 軸まわりの wrist twist としてだけ混ぜる。Hand bone は `palm.forward`
  を主軸、`palm.normal` を local `-Y` の手の平軸として作る。hand wrist の
  camera target は pose world と座標系が違うため、前腕位置の IK target としては使わない。
- VMC 側には blendshape map、bone sample construction、値の clamp/scale が別途ある。

そのため、post-process rule の評価では「UNMotionFrame/UNMF/Z の内容」と「VMC receiver
の見た目」を分けて記録する。

## 推奨 rule switch

既存の coarse switch は残したまま、desktop runtime selection の `modifier.postProcessRules` として実装済み。例:

```toml
[desktop.runtime_selection.modifier.postProcessRules]
headFromPose = true
headFromFaceMatrix = true
headReconcile = true
neutralEyeFallback = true
handCameraTarget = true
handOrientation = true
fingerDerived = true
armFromPose = true
armIkFromHands = true
crossedHandHeuristic = true
coordinateCorrection = true
finalClamp = true
```

評価用 preset はこの 3 種で足りる。

| preset | 目的 | ルール |
|---|---|---|
| `stable` | 現行互換 | 全部 on。 |
| `diagnostic-minimal` | signal 生成だけ確認 | head/hand/arm の基本変換だけ on、reconcile/fallback/IK/crossed/finger/coordinate correction を off。 |
| `vmc-compare` | VMC/Warudo 比較 | coordinate correction は on、head_reconcile/hand_camera_target/arm_ik を個別比較。 |

## 次の作業

1. `tests/pose` fixture と live capture で全 ON と `diagnostic-minimal` 相当を比較する。
2. native static regression の summary JSON には `rulePreset` と解決済み rule switches を残す。
3. rule の組み合わせ preset を GUI に追加するか判断する。

主 part スイッチとして `Head`、`Face`、`Hands`、`Arms`、`Torso`、`Legs`、`Feet` は runtime selection に入った。landmark 単位 override はまだ設計段階で、GUI には出さない。
