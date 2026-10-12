# Prism capture privacy policy slice

`prism::capture_privacy::CapturePortal` is a fixed-capacity authorization state
machine for a future screen-capture portal. An app request alone grants no
access. The trusted consent surface can show the app identity and selected
display/window generation as metadata; the module never returns framebuffer
pixels. The user can deny, allow one capture, or allow that app/source pair for
the current portal lifetime. One-shot permits are consumed exactly once and
are bound to the trusted app ID and source generation. Revocation clears
session grants and invalidates pending or issued requests for that app. Full
request/grant tables fail closed.

The caller must supply app identity from a trusted launch token and source
identity from the compositor. A window/display generation prevents consent for
an old object from following a reused numeric ID. Session grants are in-memory
only; they do not survive a portal restart.

This is a host-tested policy core, not an integrated capture service. The
current OS has no screenshot/recording data path that calls this API, no
trusted source-preview renderer, no persisted per-app permission UI, and no
camera/microphone access indicators. Until the compositor routes every capture
through this policy, screen pixels remain inaccessible through this module but
capture privacy as a system guarantee is not established. See
[`capture_privacy.rs`](../crates/prism/src/capture_privacy.rs) and the broader
privacy contract in §12.4 of the product specification.
