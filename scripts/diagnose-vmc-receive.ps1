# ============================================================================
# UN Motion VMC 受信切り分け診断 (Phase E debug)
# ============================================================================
#
# 用途:
#   Supervisor から Capturer を Launch した状態で別ターミナルで実行する。
#   「Waidayo / VSeeFace / iPad から VMC datagram は来ているか? 来ているのに
#   Capturer の `VMC receive engine received first inbound UDP datagram`
#   ログが出ない場合の切り分け」を一発で行う。
#
# 出力で何が分かるか:
#   * [TEST 1] netstat: Capturer が UDP 39540 で listen しているか
#   * [TEST 2] WindowsFirewall: un-motion-capturer.exe への受信ルール有無
#   * [TEST 3] Loopback UDP: PC から PC 自身の bind IP に UDP を送り、
#     Capturer stderr に `first inbound UDP datagram` が出るか
#     (この結果で「Capturer の受信スタック自体は健全か / OS / Firewall で
#      止まっているか」が決まる)
#   * [TEST 4] pktmon: 30 秒間 UDP 39540 のパケットを NIC レベルで監視
#     (NIC まで届いているのに Capturer に上がらないなら Firewall 確定)
#
# 使い方:
#   * Supervisor から Waidayo profile で Capturer を Launch (Capturer が
#     192.168.13.13:39540 で listen 中の状態にする)
#   * 別 PowerShell (Administrator 必要) で:
#       pwsh -File scripts\diagnose-vmc-receive.ps1 -ListenAddr 192.168.13.13:39540
#   * 表示される指示に従って Capturer 側 stderr を確認
# ============================================================================

[CmdletBinding()]
param(
	# Capturer が bind している IP:Port。Waidayo profile の
	# `vmcReceiveListenAddr` と一致させる。
	[Parameter()]
	[string]$ListenAddr = "192.168.13.13:39540",
	# pktmon の監視時間 (秒)。Waidayo を実機で動かしながら NIC まで届くかを
	# 観測する場合、ここを長めにとる。
	[Parameter()]
	[int]$PktmonSeconds = 30,
	# pktmon を skip (管理者権限 / pktmon 不可環境用)。
	[Parameter()]
	[switch]$SkipPktmon
)

$ErrorActionPreference = "Continue"
$splitIdx = $ListenAddr.LastIndexOf(":")
if ($splitIdx -lt 0) {
	Write-Error "ListenAddr must be 'IP:PORT' (got: '$ListenAddr')"
	exit 2
}
$listenIp = $ListenAddr.Substring(0, $splitIdx)
$listenPort = [int]$ListenAddr.Substring($splitIdx + 1)

function Write-Section($title) {
	Write-Host ""
	Write-Host "============================================================" -ForegroundColor Cyan
	Write-Host "  $title" -ForegroundColor Cyan
	Write-Host "============================================================" -ForegroundColor Cyan
}

# ---------------------------------------------------------------------------
# TEST 1: netstat - Capturer は listen 状態か?
# ---------------------------------------------------------------------------
Write-Section "[TEST 1] UDP $listenPort で listen しているプロセス (netstat)"
$netstat = netstat -ano | Select-String -Pattern ":$listenPort\b"
if ($netstat) {
	$netstat | ForEach-Object { Write-Host $_.Line }
	$capturerPids = @()
	foreach ($line in $netstat) {
		$parts = $line.Line -split "\s+" | Where-Object { $_ -ne "" }
		if ($parts.Count -ge 4) {
			$capturerPids += [int]$parts[-1]
		}
	}
	$capturerPids = $capturerPids | Sort-Object -Unique
	foreach ($pid_ in $capturerPids) {
		$proc = Get-Process -Id $pid_ -ErrorAction SilentlyContinue
		if ($proc) {
			Write-Host ("  PID {0} = {1} ({2})" -f $pid_, $proc.ProcessName, $proc.Path)
		}
	}
	Write-Host ""
	Write-Host "[判定 1] LISTENING 行があり PID が un-motion-capturer.exe なら正常。" -ForegroundColor Green
} else {
	Write-Host "[判定 1] UDP $listenPort で listen しているプロセスが見つからない。Capturer 未起動の可能性。" -ForegroundColor Red
}

# ---------------------------------------------------------------------------
# TEST 2: Windows Firewall - un-motion-capturer.exe の inbound ルール
# ---------------------------------------------------------------------------
Write-Section "[TEST 2] Windows Defender Firewall: un-motion-capturer.exe への inbound rule"
try {
	$capturerRules = Get-NetFirewallApplicationFilter -ErrorAction Stop |
		Where-Object { $_.Program -like "*un-motion-capturer*" }
	if ($capturerRules) {
		foreach ($f in $capturerRules) {
			$rule = $f | Get-NetFirewallRule -ErrorAction SilentlyContinue
			if ($rule) {
				Write-Host ("  DisplayName : {0}" -f $rule.DisplayName)
				Write-Host ("    Direction : {0}" -f $rule.Direction)
				Write-Host ("    Action    : {0}" -f $rule.Action)
				Write-Host ("    Profile   : {0}" -f $rule.Profile)
				Write-Host ("    Enabled   : {0}" -f $rule.Enabled)
				Write-Host ("    Program   : {0}" -f $f.Program)
				Write-Host ""
			}
		}
		Write-Host "[判定 2] Inbound + Action=Allow + Profile に現在の network が含まれていれば OK。"
		Write-Host "        Block 行がある / Inbound rule が無い場合は Firewall が原因の可能性大。" -ForegroundColor Yellow
	} else {
		Write-Host "[判定 2] un-motion-capturer.exe に対する Firewall rule が 1 つも無い。" -ForegroundColor Red
		Write-Host "        Windows 既定の挙動: 初回 inbound UDP は無音で drop されることが多い (UDP は Firewall popup が出ない)。"
		Write-Host "        対処: 管理者 PowerShell で以下を実行 (パスは適宜書き換え):"
		Write-Host '          New-NetFirewallRule -DisplayName "un-motion-capturer (UDP 39540 inbound)" `' -ForegroundColor Yellow
		Write-Host '            -Direction Inbound -Action Allow -Protocol UDP `' -ForegroundColor Yellow
		Write-Host '            -Program "$env:USERPROFILE\tmp\UNMotion\target\release\un-motion-capturer.exe" `' -ForegroundColor Yellow
		Write-Host '            -Profile Any' -ForegroundColor Yellow
	}
} catch {
	Write-Host "[判定 2] Get-NetFirewallApplicationFilter 失敗: $_" -ForegroundColor Red
	Write-Host "        管理者 PowerShell で再実行してみるか、`wf.msc` で手動確認してください。"
}

# ---------------------------------------------------------------------------
# TEST 3: Loopback UDP probe - PC から PC 自身の bind IP に UDP datagram を送る
# ---------------------------------------------------------------------------
Write-Section "[TEST 3] Loopback UDP probe: $listenIp`:$listenPort へ test datagram 送信"
Write-Host "Capturer stderr に以下が出れば PC 内 UDP スタックは健全 (= 問題は外部送信側 or NIC 通過の問題に絞れる):"
Write-Host "  un_motion_runtime::vmc_unmotion_source: VMC receive engine received first inbound UDP datagram" -ForegroundColor Green
Write-Host ""

try {
	$udp = New-Object System.Net.Sockets.UdpClient
	# /VMC/Ext/OK 1 を最小限の OSC message として送信。
	# Address pattern: "/VMC/Ext/OK" + null + (pad to 4-byte) = 12 bytes
	# Type tag:        ",i" + null + (pad to 4-byte) = 4 bytes
	# Argument:        int32(1) BE = 4 bytes
	$bytes = [byte[]]@(
		0x2F, 0x56, 0x4D, 0x43,  # "/VMC"
		0x2F, 0x45, 0x78, 0x74,  # "/Ext"
		0x2F, 0x4F, 0x4B, 0x00,  # "/OK\0"
		0x2C, 0x69, 0x00, 0x00,  # ",i\0\0"
		0x00, 0x00, 0x00, 0x01   # int32(1) BE
	)
	$sent = $udp.Send($bytes, $bytes.Length, $listenIp, $listenPort)
	Write-Host ("  Sent {0} bytes to {1}:{2}" -f $sent, $listenIp, $listenPort) -ForegroundColor Green
	$udp.Close()
	Write-Host ""
	Write-Host "Capturer stderr を確認してください。" -ForegroundColor Yellow
	Write-Host "  * `first inbound UDP datagram` が **出た**     → Capturer 受信パス健全。"
	Write-Host "    問題は iPad/Waidayo → PC NIC の経路 (Wi-Fi 帯別離 / AP isolation / iPad 送信先 IP)。"
	Write-Host "  * `first inbound UDP datagram` が **出ない**    → 同一マシン UDP loopback すら届かない。"
	Write-Host "    Windows Firewall が `un-motion-capturer.exe` への inbound UDP を block している可能性が極めて高い。"
	Write-Host "    TEST 2 の rule 追加コマンドを実行する。"
} catch {
	Write-Host "[判定 3] UDP 送信失敗: $_" -ForegroundColor Red
}

# ---------------------------------------------------------------------------
# TEST 4: pktmon - NIC レベルで UDP 39540 を観測 (管理者必須)
# ---------------------------------------------------------------------------
if ($SkipPktmon) {
	Write-Section "[TEST 4] pktmon - skip (-SkipPktmon)"
} else {
	Write-Section "[TEST 4] pktmon: NIC で UDP $listenPort のパケットを観測 ($PktmonSeconds 秒)"
	Write-Host "管理者権限が必要です。実行中に Waidayo / VSeeFace から VMC を送信してみてください。"
	Write-Host ""
	try {
		# 過去の filter を一掃
		& pktmon filter remove 2>&1 | Out-Null
		# UDP, dest port = $listenPort
		& pktmon filter add "vmc-recv-diag" -t UDP -p $listenPort 2>&1 | Out-Null
		Write-Host "pktmon start..." -ForegroundColor Yellow
		& pktmon start --capture --comp nics --pkt-size 64 2>&1 | Out-Null
		Start-Sleep -Seconds $PktmonSeconds
		& pktmon stop 2>&1 | Out-Null
		Write-Host "pktmon stop. ETL を text 化してフィルター行のみ抜粋:"
		# pktmon の出力 (text) は cwd の PktMon.txt に出力される
		$etl = Join-Path (Get-Location) "PktMon.etl"
		if (Test-Path $etl) {
			& pktmon etl2txt $etl 2>&1 | Out-Null
			$txt = Join-Path (Get-Location) "PktMon.txt"
			if (Test-Path $txt) {
				$matches = Select-String -Path $txt -Pattern "UDP" -SimpleMatch
				if ($matches) {
					$matches | Select-Object -First 50 | ForEach-Object { Write-Host $_.Line }
					Write-Host ""
					Write-Host "[判定 4] UDP packet が観測された → NIC まで届いている。" -ForegroundColor Green
					Write-Host "        Capturer ログに `first inbound UDP datagram` が出ない → Firewall block が原因。"
				} else {
					Write-Host "[判定 4] UDP packet 0 件 → そもそも NIC に届いていない。" -ForegroundColor Red
					Write-Host "        iPad の送信先 IP / Wi-Fi SSID 隔離 / NAT ルーター設定 / iPad 自身の状態 を疑う。"
				}
			}
		}
		& pktmon filter remove 2>&1 | Out-Null
	} catch {
		Write-Host "[判定 4] pktmon 失敗: $_" -ForegroundColor Red
		Write-Host "        Administrator として再実行するか、-SkipPktmon を付けてスキップしてください。"
	}
}

Write-Section "完了"
Write-Host "TEST 3 の結果が最重要です: loopback probe で `first inbound UDP datagram` が出るかを Capturer stderr で確認してください。"
