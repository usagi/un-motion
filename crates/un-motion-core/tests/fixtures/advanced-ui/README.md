# Ordinary Supervisor advanced-setting persistence fixtures

These two files are unchanged copies from the isolated normal-UI run
`un-virtual-avatar-connect/target/motion-profile-native/runs/20261022T040246064427/config/profiles/`:

- `default.toml`: `unknown-default.toml` after conflict recovery and reopening.
- `peer.toml`: `20261021T190247Z-new-profile.toml` after saving the second profile.

Paths inside these fixtures are synthetic test values. No camera or real user
configuration is represented. The test deserializes them and checks the runtime
configuration that a Capturer would receive; it does not start a Capturer,
validate FFmpeg execution, or demonstrate image/pose output.

The companion reset test removes explicit dimensions to verify that automatic
sizes follow the input dimensions and reference axis. It is a synthetic mutation
of the saved fixture, not evidence that a reset was performed in the UI.
