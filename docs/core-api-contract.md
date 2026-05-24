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
