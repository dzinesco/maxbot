# Stability repairs — September 11, 2026

This change fixes confirmed resource ownership and app integration defects:

- MCP registers response waiters before writing requests and removes them on cancellation, write failure, timeout, or completion. Dropping a server aborts its reader and stderr tasks.
- Grok retains its actual child process, kills it on final session release or explicit close, aborts background IO, and removes cancelled request waiters. Dropping a temporary clone no longer closes the shared session.
- App event subscriptions clean up even when registration partially fails or completes after unmount.
- Conversation and search loads reject stale results. Bot selection returns to chat, and new/deleted conversation handling preserves bot scope.
- Bot completion reloads persisted messages without clearing another bot's stream.
- Group chat consumes the actual text-chunk shape, retains live content only while needed, waits for group persistence before refreshing history, and prevents overlapping submissions. Group loading and failures are visible in the interface.

## Verification

- `npm test`: 201 passed across 23 files.
- `npm run build`: passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib`: 367 passed, 3 ignored.
- Added app integration tests for stale transcript routing, partial and late subscription cleanup, and group persistence/completion.
- Added regression tests for real group chunks, stream-buffer release, rapid MCP responses, repeated request cancellation, Grok clone lifetime, and final IO cleanup.

These checks use mocked frontend IPC and local backend fixtures. Live LLM providers, real Grok authentication, remote VM access, and a prolonged native-app memory profile still require a configured installation. The fixes address verified leaks; the tests do not prove that every possible leak or integration issue is eliminated.
