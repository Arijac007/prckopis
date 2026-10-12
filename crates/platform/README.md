# prckopis-platform

Layer L7 holds operating-system services that both the desktop app (`prckopis-ui-egui`) and the automation tools (`prckopis-automation`) need. Those two crates sit on the same layer and can't depend on each other, so a helper they share lives here instead of being copied into each (`xtask/src/layers.rs` allows both edges). It uses only the standard library and has no UI dependencies.

`staging` creates the temporary file an atomic save writes before renaming it over the target. The file is opened with `create_new`, so anything already at the name (a file, a hard link, a symbolic link even when dangling, a folder) is skipped and never written through. On Windows a folder at the name fails with "access denied" rather than "already exists", and is skipped only when something is actually there. Names carry an unpredictable 64-bit suffix from `staging_suffixes` (the standard library's keyed hash, currently SipHash-1-3, keyed from the operating system's random source), which stops a name being planted in advance but is not a CSPRNG; fixed suffixes are only for tests. At most 16 names are tried, and then the save is refused. The target's name is cut to 128 bytes on a character boundary so the staging name fits every system's limit. Each caller keeps its own name form (`.{name}.{suffix}.prckopis-tmp` for automation, `.{name}.prckopis-{suffix}.tmp` for the desktop save). Writing, closing the file before the rename, renaming and removing the staging file on failure stay with the caller.

Synthetic tests pin both name forms, the 128-byte cut, the attempt limit and refusal when no name is left. The planted-file, link, folder and failed-rename tests stay with each caller, where they exercise the whole save.

## Timestamp servers (`tsa`)

The transport for RFC 3161 timestamp authorities, shared by the desktop app and the automation
tools: one HTTP POST of a DER `TimeStampReq` (`application/timestamp-query`), the DER
`TimeStampResp` back, with HTTP Basic credentials when the server needs them. `prckopis-sign`
checks the reply (imprint, nonce, the TSA's signature), so plain HTTP is safe and is what most
public authorities serve. HTTPS authorities are refused with a message suggesting their http
address. Headers (64 KiB) and replies (4 MiB) are capped and at most three http redirects are
followed.
