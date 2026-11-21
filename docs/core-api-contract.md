# Core API 規約

Project CoreSplit では、UNMotion runtime を local HTTP API として公開します。現在は `un-motion-capturer.exe` が選択 profile の runtime と local Core HTTP API を所有します。

この API は control plane です。frame ごとの motion data や hot-path packet 処理を流してはいけません。

既定 bind:

```text
http://127.0.0.1:39580
```

loopback 以外の bind は、明示的な override 付きで起動された場合だけ許可します。

## runtime 操作

`GET /api/status`

現在の runtime 状態を返します。

```json
{
  "status": {
    "running": false,
    "health": "stopped",
    "activeProfileId": "default",
    "frameCount": 0,
    "packetCount": 0,
    "updatedAtMs": 0
  }
}
```

`POST /api/runtime/start`

選択中 profile の runtime を開始し、同じ status 形状を返します。

`POST /api/runtime/stop`

runtime を停止し、同じ status 形状を返します。

## profile 操作

`GET /api/profiles`

現在の runtime snapshot を返します。

```json
{
  "snapshot": {
    "status": {},
    "profiles": [
      {
        "id": "default",
        "name": "Default",
        "note": ""
      }
    ]
  }
}
```

`GET /api/profiles/active`

現在選択中の profile status を返します。

`PUT /api/profiles/active`

Request:

```json
{
  "profileId": "default"
}
```

更新後の status を返します。未知の profile は `404` です。

`PUT /api/profiles/document/sync`

Supervisor から起動中 Capturer へ profile document の内容を同期します。Capturer の現在 active profile は維持され、active profile の内容が変わった場合だけ runtime を再起動します。この endpoint は profile store へ保存しません。保存は Supervisor 側の user profile store が先に行います。

リクエストは `{"selection": <document>, "expectedProfileId": "編集対象のID"}`。Supervisor は編集対象のIDを送信します。Capturer は設定変更の排他区間内で現在の active profile と照合し、不一致なら `409 Conflict` を返して適用しません。送信先の選択が途中で変わった場合に、別対象へ設定が伝わることを防ぎます。旧クライアントとの互換性のため `expectedProfileId` は省略可能ですが、省略時はこの対象照合を行いません。同じ対象への異なる送信元からの更新順序を保証するフィールドではありません。

送信された document に現在の active profile が含まれない場合は `409 Conflict` を返し、Capturer の document・選択・runtime を変更しません。別 profile への切替は明示的な選択操作で行います。同期の拒否は、先に完了した Supervisor 側の保存を取り消すものではありません。

## event stream

`GET /api/events`

Server-Sent Events stream です。event name:

- `snapshot`
- `runtime-started`
- `runtime-stopped`
- `active-profile-changed`

各 event の `data:` payload は `CoreEvent` です。

```json
{
  "sequence": 1,
  "kind": "snapshot",
  "timestampMs": 0,
  "snapshot": {
    "status": {},
    "profiles": []
  }
}
```

telemetry event は control-plane state の sampled snapshot です。motion packet data は SSE へ流しません。

### Calibration target identity

The neutral calibration, neutral clear, and face-pose model build POST routes accept an optional `expectedProfileId` query parameter. When supplied, Core compares it with the active profile while holding the mutation lock, before any measurement or calibration update. A mismatch returns HTTP 400 with `calibration_target_conflict`; no profile switch or calibration change occurs. Omission preserves compatibility. Older Core versions do not enforce this query, so clients must deploy the updated Core for this guarantee.
