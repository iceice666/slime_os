//! Run the Rust generation decoder over a blob on disk and print its verdict.
//!
//! `slime-root` is the only production caller of `Generation::decode`, and it
//! reads bytes linked into the root image, so no host gate could reach the Rust
//! validator with a *chosen* generation -- the decoder's rules were exercised
//! only by whatever the builder happened to emit. That is enough for the happy
//! path and useless for a refusal: a rule that rejects malformed input cannot be
//! tested by a corpus that is never malformed. This example is that seam. It
//! decodes, validates, and reports the `DecodeError` variant by name, so a host
//! gate can forge a specific violation and assert the decoder names the specific
//! reason rather than merely failing somehow.
//!
//! An example rather than a test binary because `boot-contracts` already uses
//! that seam for `verify_release`, and because the gate needs to pass a path
//! chosen at run time.

use boot_contracts::generation::Generation;

fn main() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().ok_or("missing generation path")?;
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }
    let bytes = std::fs::read(&path).map_err(|error| format!("{path}: {error}"))?;
    match Generation::decode(&bytes) {
        // Printed rather than returned so the caller reads one stable token on
        // stdout either way, and a decoder panic stays distinguishable from a
        // refusal.
        Ok(_) => {
            println!("admitted");
            Ok(())
        }
        Err(error) => {
            println!("refused {error:?}");
            Ok(())
        }
    }
}
