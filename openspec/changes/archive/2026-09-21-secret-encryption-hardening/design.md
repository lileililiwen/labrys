# Design

The `seal`/`open` pair moves to AEAD with a versioned envelope
(version, nonce, ciphertext) so readers dispatch on version and
pre-rotation rows remain openable. `MasterKey` loads once from process
configuration (file or env handle, zeroized on drop) and never enters
serializable state, logs, or events. Rotation enumerates live
references, re-seals under the new key, and records approval + version
lineage in the audit trail with values excluded by construction (the
audit writer only ever sees references and versions).

Persistence writers keep their reject-plaintext-before-SQL guards, now
backed by an automated test that feeds known plaintext through every
writer and asserts absence. API and dashboard boundaries are unchanged
in shape (references only) but gain contract tests asserting no sealed
value can serialize into a response payload.
