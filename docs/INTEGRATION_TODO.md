# Integration Roadmap TODO

This tracks the integration items identified from the ZeptoClaw vs NanoBot/NanoClaw comparison.

## Status

- [x] Provider registry + runtime provider resolver
- [x] Mount allowlist validation for runtime extra mounts
- [x] Persistent cron service + `cron` tool
- [x] Channel factory/registry wiring for configured channels
- [x] Background `spawn` tool for delegated tasks

## Notes

- Keep runtime isolation opt-in and fail-closed behavior intact.
- Preserve current public APIs where possible.
- Ensure new features are additive and minimally invasive.
