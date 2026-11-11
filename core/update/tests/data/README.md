# Update test fixtures

`tools.zip` is a small stand-in for a release's tools archive: three programs, a README and a
`VERSION` file (`0.1.0-beta.5`) under `radiopost-tools-test/`; `tools-old.zip` is the same saying
`0.1.0-beta.3`. Each `.sig` is its signature and `test.pub` the public key, as Tauri's signer
writes them (`npx @tauri-apps/cli@2 signer generate`, then `signer sign`). The key is a throwaway
made for these tests: its private half was discarded, and it signs no release.
